//! Optional persistent managed WhatsApp Flow storage.
use super::RecordClient;
use std::time::Duration;

impl RecordClient {
    /// Persistent managed Flow custody is isolated from expiring interactions.
    pub(crate) fn whatsapp_flows(mut self) -> Self {
        self.record_path = "/api/v1/gateway/whatsapp-flow-records";
        self.record_kind = crate::records::RecordKind::WhatsappFlow;
        self
    }
    /// Production requires the explicitly migrated Gateway-owned collection.
    pub(crate) async fn whatsapp_flows_ready(&self) -> bool {
        let Some(database) = &self.database else {
            return false;
        };
        tokio::time::timeout(Duration::from_secs(2), database.whatsapp_flows_ready())
            .await
            .is_ok_and(|r| r.is_ok())
    }
}
