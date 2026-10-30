//! WAMI policy statements derived from a module's permissions (Track 4).
//!
//! [`statements`] is the single conversion point from a `PermDesc` (the module's policy
//! vocabulary) to the sandbox's coarse `Allow`/`Deny` statements. It exists so that
//! `this-skill-runtime` (feature `wami`) has exactly one mapping, not a second
//! `Permission` enum: a skill's permissions ARE `PolicyExpr`s.

use crate::perm::{Operation, PermDesc, PolicyExpr};
use serde::{Deserialize, Serialize};

/// A WAMI policy statement: one resource name + an allow/deny effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WamiStatement {
    /// Resource name, ARN-style: `arn:wami:skill:<tenant>:<handle>/<action>`.
    pub orn: String,
    pub effect: Effect,
}

/// Coarse grant of a sandbox statement. The fine conditions (roles, ownership, resolvers)
/// stay with the host policy engine; the sandbox only needs this gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effect {
    Allow,
    Deny,
}

/// Turn a module's permissions into sandbox statements.
///
/// Each business operation `Operation::Custom("<handle>.<action>")` yields one statement
/// on `arn:wami:skill:<tenant>:<handle>/<action>`:
/// - `PolicyExpr::Deny` → [`Effect::Deny`] (the guest may not touch the handle at all);
/// - any other policy → [`Effect::Allow`] (the host enforces the fine policy).
///
/// Fail-closed: the sandbox evaluates with *deny overrides allow* — a `Deny` statement on
/// a resource wins over any `Allow`.
pub fn statements(perm: &PermDesc, tenant: &str) -> Vec<WamiStatement> {
    perm.ops
        .iter()
        .filter_map(|(op, policy)| {
            let Operation::Custom(name) = op else {
                return None;
            };
            let (handle, action) = name.split_once('.')?;
            let effect = match policy {
                PolicyExpr::Deny => Effect::Deny,
                _ => Effect::Allow,
            };
            Some(WamiStatement {
                orn: format!("arn:wami:skill:{tenant}:{handle}/{action}"),
                effect,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::perm::{Operation, PolicyExpr};

    fn perm(ops: &[(&str, &str)]) -> PermDesc {
        let mut p = PermDesc::default();
        for (op, policy) in ops {
            p.ops.insert(
                op.parse::<Operation>().unwrap(),
                PolicyExpr::parse(policy).unwrap(),
            );
        }
        p
    }

    #[test]
    fn each_handle_produces_an_orn_and_effect() {
        let p = perm(&[
            ("graph.read", "authenticated"),
            ("inference.run", "perm:inference.run"),
            ("storage.write", "deny"),
        ]);
        let stmts = statements(&p, "acme");
        assert_eq!(stmts.len(), 3);
        assert!(stmts.contains(&WamiStatement {
            orn: "arn:wami:skill:acme:graph/read".into(),
            effect: Effect::Allow,
        }));
        assert!(stmts.contains(&WamiStatement {
            orn: "arn:wami:skill:acme:inference/run".into(),
            effect: Effect::Allow,
        }));
        assert!(stmts.contains(&WamiStatement {
            orn: "arn:wami:skill:acme:storage/write".into(),
            effect: Effect::Deny,
        }));
    }

    #[test]
    fn standard_operations_produce_no_statement() {
        // CRUD ops (list/get/create/…) are not sandbox handles: they produce nothing.
        let p = perm(&[("list", "public"), ("create", "role:admin")]);
        assert!(statements(&p, "acme").is_empty());
    }

    #[test]
    fn deny_primes_over_allow() {
        // Same handle, one allowed action and one denied action: both statements exist and
        // the deny is what the sandbox enforces (deny overrides allow).
        let p = perm(&[("graph.read", "authenticated"), ("graph.write", "deny")]);
        let stmts = statements(&p, "acme");
        assert!(stmts.iter().any(|s| s.effect == Effect::Deny));
        assert!(stmts.iter().any(|s| s.effect == Effect::Allow));
    }
}
