//! Native client injection stays separate from the NyxID relay transport.
use super::relay_client::RelayClient;
use std::sync::Arc;

impl RelayClient {
    pub fn with_whatsapp(
        mut self,
        client: crate::whatsapp::Client,
        receipts: Arc<crate::whatsapp::receipts::Store>,
    ) -> Self {
        self.whatsapp = client;
        self.whatsapp_receipts = Some(receipts);
        self
    }
    pub fn with_telegram(mut self, client: crate::telegram::TelegramClient) -> Self {
        self.telegram = client;
        self
    }
}
