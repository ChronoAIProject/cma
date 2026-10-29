//! Bounded ordinary native transport capabilities.
use super::Capabilities;

impl Capabilities {
    pub fn native_discord() -> Self {
        Self {
            max_text_chars: 2000,
            ..Self::plain_text()
        }
    }
    pub fn native_lark() -> Self {
        Self {
            max_text_chars: 6000,
            ..Self::plain_text()
        }
    }
    pub fn native_whatsapp() -> Self {
        Self {
            max_text_chars: 4096,
            ..Self::plain_text()
        }
    }
}

impl super::ToolsState {
    pub(crate) async fn structured_reply_capability(
        &self,
        caller: crate::auth::AgentCaller,
        channel: &crate::channels::resolver::ResolvedChannel,
        event: &cmaeg_core::seal::EventRefPayload,
    ) -> Option<Vec<String>> {
        if !self.native_telegram_structured_replies_enabled
            || !matches!(event.reply, cmaeg_core::seal::ReplyAddress::Telegram { .. })
        {
            return None;
        }
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            let current = self
                .resolver
                .resolve_current(channel.id.as_str())
                .await
                .ok()?;
            let selected = event
                .consumer
                .clone()
                .unwrap_or_else(cmaeg_core::ids::ConsumerId::default_consumer);
            if caller.consumer(&current).ok()? != selected {
                return None;
            }
            crate::telegram::structured::authorized(
                self.resolver.record_client(),
                self.keys.clone(),
                &current,
                event,
            )
            .await
            .then(|| ["contact", "location", "venue"].map(String::from).into())
        })
        .await
        .ok()
        .flatten()
    }

    pub(crate) async fn received_sticker_capability(
        &self,
        caller: crate::auth::AgentCaller,
        channel: &crate::channels::resolver::ResolvedChannel,
        event: &cmaeg_core::seal::EventRefPayload,
        verified: bool,
    ) -> Option<bool> {
        if !verified
            || !self.native_lark_sticker_replies_enabled
            || !matches!(event.reply, cmaeg_core::seal::ReplyAddress::NativeLark(_))
        {
            return None;
        }
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            let current = self
                .resolver
                .resolve_current(channel.id.as_str())
                .await
                .ok()?;
            let selected = event
                .consumer
                .clone()
                .unwrap_or_else(cmaeg_core::ids::ConsumerId::default_consumer);
            if caller.consumer(&current).ok()? != selected {
                return None;
            }
            crate::native_lark::stickers::load(
                self.resolver.record_client(),
                self.keys.clone(),
                &current,
                event,
            )
            .await
            .map(|_| true)
        })
        .await
        .ok()
        .flatten()
    }
}
