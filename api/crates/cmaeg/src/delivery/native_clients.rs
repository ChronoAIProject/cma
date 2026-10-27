//! Native client injection stays separate from the NyxID relay transport.
use super::relay_client::RelayClient;
use std::sync::Arc;

impl RelayClient {
    pub fn with_native_lark_outbound_media(mut self, enabled: bool) -> Self {
        self.native_lark_outbound_media = enabled;
        self
    }
    pub(super) fn native_lark_outbound_media_enabled(&self) -> bool {
        self.native_lark_outbound_media
    }

    pub fn with_native_lark(
        mut self,
        client: crate::native_lark::Client,
        receipts: Arc<crate::native_lark::receipts::Store>,
    ) -> Self {
        self.native_lark = client;
        self.native_lark_receipts = Some(receipts);
        self
    }
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
