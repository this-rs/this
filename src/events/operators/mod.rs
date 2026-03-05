//! Pipeline operators for declarative event flows
//!
//! Each operator implements the `PipelineOperator` trait and processes a
//! `FlowContext` during pipeline execution. Operators are compiled from
//! YAML `PipelineStep` configurations.
//!
//! # Operator types
//!
//! **Synchronous (1:1)** — preserve cardinality:
//! - `resolve` — Resolve an entity by ID or by following a link
//! - `filter` — Drop events that don't match a condition
//! - `map` — Transform the payload via a Tera template
//! - `deliver` — Send to one or more sinks
//!
//! **Stateful (1:N or N:1)** — change cardinality:
//! - `fan_out` — Multiply event for each linked entity (see T2.3)
//! - `batch` — Accumulate events and flush on window expiry (see T2.3)
//! - `deduplicate` — Remove duplicates within a window (see T2.3)
//! - `rate_limit` — Throttle via token bucket (see T2.3)

pub mod deliver;
pub mod filter;
pub mod map;
pub mod resolve;

pub use deliver::DeliverOp;
pub use filter::FilterOp;
pub use map::MapOp;
pub use resolve::ResolveOp;

use crate::events::context::FlowContext;
use anyhow::Result;
use async_trait::async_trait;

/// Result of executing a pipeline operator
#[derive(Debug)]
pub enum OpResult {
    /// Continue to the next operator in the pipeline
    Continue,

    /// Drop this event — stop pipeline execution for this event
    Drop,

    /// Fan out into multiple contexts (one per element)
    ///
    /// Each resulting FlowContext will continue through the remaining
    /// pipeline operators independently.
    FanOut(Vec<FlowContext>),
}

/// Trait for pipeline operators
///
/// Each operator receives a mutable `FlowContext` and returns an `OpResult`
/// indicating whether to continue, drop, or fan out.
///
/// # Implementors
///
/// - `ResolveOp` — resolves entities via LinkService/EntityFetcher
/// - `FilterOp` — evaluates boolean conditions
/// - `MapOp` — transforms payload via Tera templates
/// - `DeliverOp` — delivers to sinks
#[async_trait]
pub trait PipelineOperator: Send + Sync + std::fmt::Debug {
    /// Execute this operator on the given context
    ///
    /// May modify the context (e.g., adding variables) and returns
    /// an `OpResult` indicating how to proceed.
    async fn execute(&self, ctx: &mut FlowContext) -> Result<OpResult>;

    /// Human-readable name for logging/debugging
    fn name(&self) -> &str;
}
