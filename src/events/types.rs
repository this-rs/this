//! Core types for the event log system

use serde::{Deserialize, Serialize};

/// Sequence number in the event log (monotonically increasing)
pub type SeqNo = u64;

/// Seek position for event log consumers
///
/// Determines where a consumer starts reading from when subscribing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SeekPosition {
    /// Start from the very beginning — replay all events
    Beginning,
    /// Resume from the last acknowledged position for this consumer
    LastAcknowledged,
    /// Start from now — only receive future events
    Latest,
    /// Start from a specific sequence number
    Sequence(SeqNo),
}

impl Default for SeekPosition {
    fn default() -> Self {
        SeekPosition::Latest
    }
}

impl From<crate::config::SeekMode> for SeekPosition {
    fn from(mode: crate::config::SeekMode) -> Self {
        match mode {
            crate::config::SeekMode::Beginning => SeekPosition::Beginning,
            crate::config::SeekMode::LastAcknowledged => SeekPosition::LastAcknowledged,
            crate::config::SeekMode::Latest => SeekPosition::Latest,
        }
    }
}

/// State of a consumer group
#[derive(Debug, Clone)]
pub struct ConsumerState {
    /// Consumer group name
    pub name: String,
    /// Last acknowledged sequence number (None if never acked)
    pub last_acked: Option<SeqNo>,
}
