//! Native client injection stays separate from the NyxID relay transport.
use super::relay_client::RelayClient;
use std::sync::Arc;

impl RelayClient {
    /// Enable only after every serving and rollback reader supports voice/animation kinds.
    pub fn with_native_media_presentations(mut self, enabled: bool) -> Self {
        self.native_media_presentations = enabled;
        self
    }

    pub fn with_native_lark_media_card_replacements(mut self, enabled: bool) -> Self {
        self.native_lark_media_card_replacements = enabled;
        self
    }

    pub fn with_native_telegram_albums(mut self, enabled: bool) -> Self {
        self.native_telegram_albums = enabled;
        self
    }

    pub fn with_native_lark_thread_start(mut self, enabled: bool) -> Self {
        self.native_lark_thread_start = enabled;
        self
    }
    pub fn with_native_lark_multi_image_controls(mut self, enabled: bool) -> Self {
        self.native_lark_multi_image_controls = enabled;
        self
    }
    pub fn with_native_lark_media_controls(mut self, enabled: bool) -> Self {
        self.native_lark_media_controls = enabled;
        self
    }
    /// Enable new receipt fields only after every retained-receipt reader is deployed.
    pub fn with_native_split_text_refs(mut self, enabled: bool) -> Self {
        self.native_split_text_refs = enabled;
        self
    }

    pub fn with_native_lark_controls(mut self, controls: bool, forms: bool) -> Self {
        self.native_lark_controls = controls;
        self.native_lark_forms = controls && forms;
        self
    }
    pub fn with_native_lark_effects(mut self, enabled: bool) -> Self {
        self.native_lark_effects = enabled;
        self
    }
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

impl super::relay_client::RelayClient {
    pub fn with_native_media_edits(
        mut self,
        telegram: bool,
        discord: bool,
        store: std::sync::Arc<crate::native_media::store::Store>,
    ) -> Self {
        self.telegram_media_edits = telegram;
        self.discord_media_edits = discord;
        self.native_media_receipts = Some(store);
        self
    }
}
