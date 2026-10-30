//! Execution capabilities of a WASM module (Track 4): the `runtime` section of a
//! `ModuleDescriptor`.
//!
//! A declarative or compiled module has `runtime: None`. A module that ships guest code
//! (a WASM skill) declares `runtime: Some(RuntimeDesc)` — the exported entry point, the
//! capabilities (handles) it requests, and the resource limits of its sandbox. The
//! permissions governing those handles stay in `PermDesc` (one vocabulary, no second
//! manifest): [`crate::wami::statements`] turns them into sandbox policy statements.

use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// How to run this module as a WASM skill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeDesc {
    /// Exported WASM entry point (function name).
    pub entry: String,
    /// Capabilities the guest requests. Each is gated by a WAMI permission derived from
    /// the module's `PermDesc`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub handles: Vec<Handle>,
    /// Resource limits enforced by the sandbox.
    #[serde(default)]
    pub limits: RuntimeLimits,
}

/// A capability a WASM module can request. Serialized as a string (`"graph"`,
/// `"inference"`, `"storage"`, `"network"`, or a custom dotted name).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub enum Handle {
    Graph,
    Inference,
    Storage,
    Network,
    Custom(String),
}

impl Handle {
    pub const STANDARD: [Handle; 4] = [
        Handle::Graph,
        Handle::Inference,
        Handle::Storage,
        Handle::Network,
    ];

    pub fn as_str(&self) -> &str {
        match self {
            Handle::Graph => "graph",
            Handle::Inference => "inference",
            Handle::Storage => "storage",
            Handle::Network => "network",
            Handle::Custom(s) => s,
        }
    }
}

impl std::fmt::Display for Handle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Invalid handle name.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid handle `{0}`: expected graph, inference, storage, network or a dotted name")]
pub struct InvalidHandle(pub String);

impl FromStr for Handle {
    type Err = InvalidHandle;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(h) = Handle::STANDARD.iter().find(|h| h.as_str() == s) {
            return Ok(h.clone());
        }
        let ok = s.contains('.') && s.split('.').all(crate::naming::is_identifier);
        if ok {
            Ok(Handle::Custom(s.to_string()))
        } else {
            Err(InvalidHandle(s.to_string()))
        }
    }
}

impl From<Handle> for String {
    fn from(h: Handle) -> String {
        h.as_str().to_string()
    }
}

impl TryFrom<String> for Handle {
    type Error = InvalidHandle;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

/// Resource limits of the sandbox. All optional; `None`/`false` = the host's defaults.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RuntimeLimits {
    /// Max guest linear memory, in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_bytes: Option<u64>,
    /// Max CPU time per invocation, in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_time_ms: Option<u64>,
    /// Whether the guest may open network sockets.
    #[serde(default)]
    pub network: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_roundtrip() {
        for (s, h) in [
            ("graph", Handle::Graph),
            ("inference", Handle::Inference),
            ("storage", Handle::Storage),
            ("network", Handle::Network),
            ("custom.thing", Handle::Custom("custom.thing".into())),
        ] {
            assert_eq!(s.parse::<Handle>().unwrap(), h);
            assert_eq!(h.to_string(), s);
        }
        assert!("not-an-id".parse::<Handle>().is_err());
    }

    #[test]
    fn runtime_section_roundtrip() {
        let r = RuntimeDesc {
            entry: "run".into(),
            handles: vec![Handle::Graph, Handle::Inference],
            limits: RuntimeLimits {
                memory_bytes: Some(1 << 20),
                cpu_time_ms: Some(50),
                network: false,
            },
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: RuntimeDesc = serde_json::from_str(&json).unwrap();
        assert_eq!(back, r);
    }
}
