//! Event bridge between this-rs EventBus and obrain-db MutationBus
//!
//! Provides bidirectional event translation:
//! - **Outbound** (T8): `FrameworkEvent` → `MutationEvent` (for obrain cognitive listeners)
//! - **Inbound** (T9): `MutationEvent` → `FrameworkEvent` (for this-rs clients via WS/SSE/GraphQL)
//!
//! All types and logic are feature-gated behind `#[cfg(feature = "obrain")]`.

#[cfg(feature = "obrain")]
pub mod types;
#[cfg(feature = "obrain")]
pub mod outbound;
#[cfg(feature = "obrain")]
pub mod inbound;
#[cfg(feature = "obrain")]
pub mod dedup;
pub mod cognitive;

#[cfg(feature = "obrain")]
pub use outbound::OutboundBridge;
#[cfg(feature = "obrain")]
pub use inbound::InboundBridge;
#[cfg(feature = "obrain")]
pub use dedup::DedupFilter;
pub use cognitive::{CognitiveNotificationBridge, CognitiveNotificationConfig};
