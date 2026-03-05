//! Persistent event log system for declarative event flows
//!
//! This module provides the `EventLog` trait and implementations for
//! durable event storage, replacing the fire-and-forget `EventBus` as the
//! source of truth for event flows.
//!
//! # Architecture
//!
//! ```text
//! EventBus (broadcast, real-time)
//!     ↓ bridge
//! EventLog (persistent, ordered, replayable)
//!     ↓ subscribe
//! FlowRuntime (consumes events, executes pipelines)
//! ```
//!
//! # Backends
//!
//! - `InMemoryEventLog` — Default, suitable for development and single-instance
//! - Future: NATS JetStream, Kafka, Redis Streams

pub mod context;
pub mod log;
pub mod matcher;
pub mod memory;
pub mod types;

pub use context::FlowContext;
pub use log::EventLog;
pub use matcher::EventMatcher;
pub use memory::InMemoryEventLog;
pub use types::*;
