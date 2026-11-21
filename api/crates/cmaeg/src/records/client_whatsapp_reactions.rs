//! Native reaction receipts and fences preserve their original interaction IDs and seals.
use super::RecordClient;

impl RecordClient {
    pub(crate) fn whatsapp_reaction_records(self) -> Self {
        let mut records = self.interactions();
        records.retention = super::super::retention::InteractionRetention::WhatsappReaction;
        records
    }

    pub(crate) fn whatsapp_reaction_storage_available(&self) -> bool {
        #[cfg(test)]
        if self.whatsapp_reaction_fixture {
            return true;
        }
        self.database.is_some()
    }

    /// Explicit typed-storage fixture; never relaxes the ordinary HTTP route.
    #[cfg(test)]
    pub(crate) fn with_whatsapp_reaction_fixture(mut self) -> Self {
        self.whatsapp_reaction_fixture = true;
        self
    }
}
