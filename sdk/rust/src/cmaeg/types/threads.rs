//! Explicit opt-in native-thread policy for CMAEG channels.
use serde::{Deserialize, Serialize};

/// Opt-in native-thread metadata and bounded per-chat admission.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ThreadContract {
    /// Negotiated contract version (currently 1).
    pub version: u16,
    #[serde(default)]
    /// At most 256 unique chat IDs admitted for consumer-owned follow policy.
    pub follow_chat_ids: Vec<String>,
}
