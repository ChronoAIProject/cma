//! The GitHub source write gate for offline record import.
//!
//! `--import-legacy-records` is a channel writer like any management write,
//! so while `CMAEG_GITHUB_ENABLED` is off it may not introduce a GitHub source.
//! Import never overwrites a row: a channel row the destination already holds
//! byte-for-byte is a no-op retention, and every other channel row is a create.
//! A create is opened with the gateway key ring and checked by the same
//! policy as management writes. A row that cannot be opened is refused, so an
//! export the gate cannot read never reaches storage while the flag is off.
use std::sync::Arc;

use cmaeg_core::{
    channel::ChannelDefinition,
    seal::{open_retained_channel, MasterKeyRing},
};

use super::RecordError;

/// Refusal of a channel row that would introduce a GitHub source while disabled.
pub const GITHUB_IMPORT_DISABLED: &str =
    "import refused: a channel row adds a GitHub source while CMAEG_GITHUB_ENABLED=false";
/// Refusal of a channel row the disabled gate has no key ring to inspect.
pub const GITHUB_IMPORT_KEYS_REQUIRED: &str =
    "import refused: CMAEG_GITHUB_ENABLED=false requires the gateway key ring to inspect channel rows";
/// Refusal of a channel row the disabled gate cannot open or read.
pub const GITHUB_IMPORT_UNREADABLE: &str =
    "import refused: a channel row cannot be opened to check its sources";

/// The channel write policy an import applies before storing a row.
#[derive(Clone)]
pub struct ImportGate {
    github_enabled: bool,
    keys: Option<Arc<MasterKeyRing>>,
}

impl ImportGate {
    /// An explicit policy, for callers that already hold the flag and key ring.
    pub fn new(github_enabled: bool, keys: Option<MasterKeyRing>) -> Self {
        Self {
            github_enabled,
            keys: keys.map(Arc::new),
        }
    }

    /// The deployed policy: the Gateway's GitHub write flag and key ring.
    ///
    /// An absent key ring is not an error here, because an export without
    /// channel rows needs none; the first channel row that must be inspected
    /// reports it. A configured but invalid ring is refused immediately.
    pub fn from_environment() -> Result<Self, RecordError> {
        let github_enabled = crate::config::github_writes_enabled().map_err(|_| {
            RecordError::Rejected("CMAEG_GITHUB_ENABLED must be true or false".into())
        })?;
        let configured = std::env::var_os(crate::config::CMAEG_MASTER_KEYS).is_some()
            || std::env::var_os(crate::CMAEG_MASTER_KEYS_FILE).is_some();
        let keys = if github_enabled || !configured {
            None
        } else {
            let spec = crate::config::secrets::read(
                crate::config::CMAEG_MASTER_KEYS,
                crate::CMAEG_MASTER_KEYS_FILE,
            )
            .map_err(|_| RecordError::Rejected("gateway key ring is unreadable".into()))?;
            Some(
                MasterKeyRing::parse(&spec)
                    .map_err(|_| RecordError::Rejected("gateway key ring is invalid".into()))?,
            )
        };
        Ok(Self::new(github_enabled, keys))
    }

    /// Refuse a sealed channel row that the destination does not already
    /// hold unchanged and that introduces a GitHub source while disabled.
    pub(super) fn check_new_channel(
        &self,
        record_id: &str,
        version: u64,
        blob: &[u8],
    ) -> Result<(), RecordError> {
        if self.github_enabled {
            return Ok(());
        }
        let keys = self
            .keys
            .as_deref()
            .ok_or_else(|| RecordError::Rejected(GITHUB_IMPORT_KEYS_REQUIRED.into()))?;
        let unreadable = || RecordError::Rejected(GITHUB_IMPORT_UNREADABLE.into());
        let (opened, _) =
            open_retained_channel(keys, record_id, version, blob).map_err(|_| unreadable())?;
        let definition: ChannelDefinition =
            serde_json::from_slice(&opened.payload).map_err(|_| unreadable())?;
        // Import is insert-only, so a row it may store has no current definition.
        crate::channels::check_github_sources(None, &definition, false)
            .map_err(|_| RecordError::Rejected(GITHUB_IMPORT_DISABLED.into()))
    }
}

#[cfg(test)]
#[path = "import_gate_tests.rs"]
mod tests;
