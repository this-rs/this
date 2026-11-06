//! The spending ledger: tokens and micro-dollars per provider per day, reserved before
//! a call and settled after it.

use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::config::{ProviderConfig, ProviderId};

/// What time it is, in whole UTC days since the Unix epoch.
///
/// A trait so the same ledger runs on a server (`SystemClock`) and in a browser
/// wasm, where `SystemTime::now` is not available: the page gives the day.
pub trait Clock: Send + Sync {
    fn day(&self) -> u64;
}

/// The machine's clock. Native targets only: `SystemTime::now` panics on
/// `wasm32-unknown-unknown`.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn day(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs() / 86_400)
            .unwrap_or(0)
    }
}

/// What a call consumed, or is expected to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Priced cost, micro-dollars. Zero for a provider without pricing.
    pub cost_micros: u64,
}

impl Usage {
    pub fn tokens(&self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }

    fn plus(self, other: Usage) -> Usage {
        Usage {
            input_tokens: self.input_tokens.saturating_add(other.input_tokens),
            output_tokens: self.output_tokens.saturating_add(other.output_tokens),
            cost_micros: self.cost_micros.saturating_add(other.cost_micros),
        }
    }

    fn minus(self, other: Usage) -> Usage {
        Usage {
            input_tokens: self.input_tokens.saturating_sub(other.input_tokens),
            output_tokens: self.output_tokens.saturating_sub(other.output_tokens),
            cost_micros: self.cost_micros.saturating_sub(other.cost_micros),
        }
    }
}

/// Which daily limit a call would cross.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BudgetError {
    Tokens { limit: u64, spent: u64, asked: u64 },
    Cost { limit: u64, spent: u64, asked: u64 },
}

impl fmt::Display for BudgetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tokens {
                limit,
                spent,
                asked,
            } => write!(
                formatter,
                "budget de tokens du jour atteint : {spent} consommés, {asked} demandés, plafond {limit}"
            ),
            Self::Cost {
                limit,
                spent,
                asked,
            } => write!(
                formatter,
                "budget du jour atteint : {} $ dépensés, {} $ demandés, plafond {} $",
                micros_as_dollars(*spent),
                micros_as_dollars(*asked),
                micros_as_dollars(*limit),
            ),
        }
    }
}

impl std::error::Error for BudgetError {}

fn micros_as_dollars(micros: u64) -> String {
    format!("{:.4}", micros as f64 / 1_000_000.0)
}

/// Usage per provider per day. In memory: a process that restarts starts the day again,
/// which is stated here so nobody mistakes it for a ledger of record.
#[derive(Debug, Default)]
pub struct Ledger {
    days: HashMap<(ProviderId, u64), Usage>,
}

impl Ledger {
    /// What `id` has consumed on `day`, reservations included.
    pub fn spent(&self, id: &ProviderId, day: u64) -> Usage {
        self.days
            .get(&(id.clone(), day))
            .copied()
            .unwrap_or_default()
    }

    /// Checks the daily limits against what is already spent plus `estimate`, and if
    /// they hold, reserves `estimate`. Refused calls reserve nothing.
    pub fn reserve(
        &mut self,
        config: &ProviderConfig,
        day: u64,
        estimate: Usage,
    ) -> Result<(), BudgetError> {
        // Days before today are no longer read: drop them so the map stays small.
        self.days.retain(|(_, stored), _| *stored >= day);

        let spent = self.spent(&config.id, day);
        if let Some(limit) = config.budget.max_tokens_per_day {
            let after = spent.tokens().saturating_add(estimate.tokens());
            if after > limit {
                return Err(BudgetError::Tokens {
                    limit,
                    spent: spent.tokens(),
                    asked: estimate.tokens(),
                });
            }
        }
        if let Some(limit) = config.budget.max_cost_micros_per_day {
            let after = spent.cost_micros.saturating_add(estimate.cost_micros);
            if after > limit {
                return Err(BudgetError::Cost {
                    limit,
                    spent: spent.cost_micros,
                    asked: estimate.cost_micros,
                });
            }
        }
        self.add(&config.id, day, estimate);
        Ok(())
    }

    /// Replaces a reservation by what the engine actually consumed.
    pub fn settle(&mut self, id: &ProviderId, day: u64, reserved: Usage, actual: Usage) {
        self.sub(id, day, reserved);
        self.add(id, day, actual);
    }

    /// Gives back a reservation for a call that never completed.
    pub fn release(&mut self, id: &ProviderId, day: u64, reserved: Usage) {
        self.sub(id, day, reserved);
    }

    fn add(&mut self, id: &ProviderId, day: u64, usage: Usage) {
        let entry = self.days.entry((id.clone(), day)).or_default();
        *entry = entry.plus(usage);
    }

    fn sub(&mut self, id: &ProviderId, day: u64, usage: Usage) {
        let entry = self.days.entry((id.clone(), day)).or_default();
        *entry = entry.minus(usage);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Budget;
    use this_llm::{CredentialRef, Preset};

    fn provider(budget: Budget) -> ProviderConfig {
        let mut config = ProviderConfig::new(
            ProviderId::new("vllm").unwrap(),
            "vLLM",
            Preset::Vllm,
            CredentialRef::None,
        );
        config.budget = budget;
        config
    }

    fn usage(tokens: u64, cost: u64) -> Usage {
        Usage {
            input_tokens: tokens,
            output_tokens: 0,
            cost_micros: cost,
        }
    }

    #[test]
    fn a_token_limit_refuses_the_call_that_would_cross_it() {
        let config = provider(Budget {
            max_tokens_per_day: Some(100),
            ..Budget::default()
        });
        let mut ledger = Ledger::default();
        assert!(ledger.reserve(&config, 1, usage(60, 0)).is_ok());
        assert_eq!(
            ledger.reserve(&config, 1, usage(60, 0)),
            Err(BudgetError::Tokens {
                limit: 100,
                spent: 60,
                asked: 60
            }),
            "la seconde demande dépasse : refusée, rien réservé"
        );
        assert_eq!(ledger.spent(&config.id, 1).tokens(), 60);
    }

    #[test]
    fn a_reservation_is_settled_to_the_real_usage() {
        let config = provider(Budget::default());
        let mut ledger = Ledger::default();
        let estimate = usage(1000, 0);
        ledger.reserve(&config, 1, estimate).unwrap();
        ledger.settle(&config.id, 1, estimate, usage(120, 0));
        assert_eq!(ledger.spent(&config.id, 1).tokens(), 120);
    }

    #[test]
    fn a_failed_call_gives_its_reservation_back() {
        let config = provider(Budget::default());
        let mut ledger = Ledger::default();
        ledger.reserve(&config, 1, usage(500, 0)).unwrap();
        ledger.release(&config.id, 1, usage(500, 0));
        assert_eq!(ledger.spent(&config.id, 1), Usage::default());
    }

    #[test]
    fn a_cost_limit_is_enforced_in_micro_dollars() {
        let config = provider(Budget {
            max_cost_micros_per_day: Some(1_000_000),
            ..Budget::default()
        });
        let mut ledger = Ledger::default();
        assert!(ledger.reserve(&config, 1, usage(0, 900_000)).is_ok());
        assert!(matches!(
            ledger.reserve(&config, 1, usage(0, 200_000)),
            Err(BudgetError::Cost { .. })
        ));
    }

    #[test]
    fn each_day_starts_with_a_clean_budget() {
        let config = provider(Budget {
            max_tokens_per_day: Some(10),
            ..Budget::default()
        });
        let mut ledger = Ledger::default();
        ledger.reserve(&config, 1, usage(10, 0)).unwrap();
        assert!(ledger.reserve(&config, 1, usage(1, 0)).is_err());
        assert!(ledger.reserve(&config, 2, usage(10, 0)).is_ok());
    }
}
