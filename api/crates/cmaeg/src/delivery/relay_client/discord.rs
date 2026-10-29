//! Reader-first native Discord feature issuance; retained references stay readable.
use super::RelayClient;

impl RelayClient {
    pub fn with_discord_media_card_sets(mut self, enabled: bool) -> Self {
        self.discord_media_card_sets = enabled;
        self
    }
    pub fn with_discord_reactions(mut self, enabled: bool) -> Self {
        self.discord_reactions = enabled;
        self
    }
    pub(in crate::delivery) fn discord_reactions_enabled(&self) -> bool {
        self.discord_reactions
    }
    pub fn with_discord_forms(mut self, enabled: bool) -> Self {
        self.discord_forms = enabled;
        self
    }
    pub(in crate::delivery) fn discord_forms_enabled(&self) -> bool {
        self.discord_forms
    }
    pub fn with_discord_card_edits(mut self, enabled: bool) -> Self {
        self.discord_card_edits = enabled;
        self
    }
    pub(in crate::delivery) fn discord_card_edits_enabled(&self) -> bool {
        self.discord_card_edits
    }
    pub fn with_native_discord_effects(mut self, enabled: bool) -> Self {
        self.native_discord_effects = enabled;
        self
    }
    pub(in crate::delivery) fn native_discord_effects_enabled(&self) -> bool {
        self.native_discord_effects
    }
    /// Issue effects-only media references after all retained readers support them.
    pub fn with_native_discord_media_effects(mut self, enabled: bool) -> Self {
        self.native_discord_media_effects = enabled;
        self
    }
    pub(in crate::delivery) fn native_discord_media_effects_enabled(&self) -> bool {
        self.native_discord_media_effects
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
    /// New media-bearing cards need a compatible retained reader before issuance.
    /// Disabling this flag does not block callbacks to already bound cards.
    pub fn with_discord_media_controls(mut self, enabled: bool) -> Self {
        self.discord_media_controls = enabled;
        self
    }
    pub(in crate::delivery) fn discord_media_controls_enabled(&self) -> bool {
        self.discord_media_controls
    }
    /// Issue confirmed media-preserving edit authority after compatible readers deploy.
    pub fn with_discord_media_card_edits(mut self, enabled: bool) -> Self {
        self.discord_media_card_edits = enabled;
        self
    }
    pub(in crate::delivery) fn discord_media_card_edits_enabled(&self) -> bool {
        self.discord_media_card_edits
    }
    /// Pending replacement readers must deploy before new upload replacement issuance.
    pub fn with_discord_media_card_replacements(mut self, enabled: bool) -> Self {
        self.discord_media_card_replacements = enabled;
        self
    }
    pub(in crate::delivery) fn discord_media_card_replacements_enabled(&self) -> bool {
        self.discord_media_card_replacements && self.discord_media_card_edits
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
