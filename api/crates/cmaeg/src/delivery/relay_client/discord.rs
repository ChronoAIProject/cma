//! Reader-first native Discord feature issuance; retained references stay readable.
use super::RelayClient;

impl RelayClient {
    pub fn with_native_discord_effects(mut self, enabled: bool) -> Self {
        self.native_discord_effects = enabled;
        self
    }
    pub(in crate::delivery) fn native_discord_effects_enabled(&self) -> bool {
        self.native_discord_effects
    }
    /// Issue native edit references only after every reader supports their authority.
    /// Disabling issuance does not revoke already issued references.
    pub fn with_native_discord_edits(mut self, enabled: bool) -> Self {
        self.native_discord_edits = enabled;
        self
    }
    pub(in crate::delivery) fn native_discord_edits_enabled(&self) -> bool {
        self.native_discord_edits
    }
    pub fn with_discord_interactions(mut self, enabled: bool) -> Self {
        self.discord_interactions = enabled;
        self
    }
    pub(in crate::delivery) fn discord_interactions_enabled(&self) -> bool {
        self.discord_interactions
    }
    /// Gate only new multi-select cards; persisted selection recovery stays available.
    pub fn with_discord_multiselect(mut self, enabled: bool) -> Self {
        self.discord_multiselect = enabled;
        self
    }
    pub(in crate::delivery) fn discord_multiselect_enabled(&self) -> bool {
        self.discord_multiselect
    }
}
