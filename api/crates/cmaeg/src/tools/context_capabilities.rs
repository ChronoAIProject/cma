//! Baseline capabilities projected for each supported delivery transport.
use super::{Capabilities, ToolsState};
use crate::{auth::AgentCaller, channels::resolver::ResolvedChannel};
use cmaeg_core::channel::Platform;
use cmaeg_core::seal::EventRefPayload;

impl Capabilities {
    pub fn custom() -> Self {
        Self {
            edit: false,
            typing: false,
            forms: None,
            buttons: "json".into(),
            cards: "json".into(),
            attachments: "reference".into(),
            max_text_chars: 32768,
            inbound_media: None,
            native_media: None,
            native_media_edits: None,
            whatsapp_templates: None,
            whatsapp_template_components: None,
            message_effects: None,
            thread_history: None,
            topic_lifecycle: None,
            topic_creation: None,
            thread_lifecycle: None,
            native_thread_start: None,
            structured_replies: None,
            reply_with_received_sticker: None,
            conversation_history: None,
        }
    }
    pub fn native_telegram() -> Self {
        Self {
            typing: true,
            ..Self::telegram()
        }
    }
    pub fn telegram() -> Self {
        Self {
            edit: true,
            typing: false,
            forms: None,
            buttons: "inline_keyboard".into(),
            cards: "text".into(),
            attachments: "link_text".into(),
            max_text_chars: 4000,
            inbound_media: None,
            native_media: None,
            native_media_edits: None,
            whatsapp_templates: None,
            whatsapp_template_components: None,
            message_effects: None,
            thread_history: None,
            topic_lifecycle: None,
            topic_creation: None,
            thread_lifecycle: None,
            native_thread_start: None,
            structured_replies: None,
            reply_with_received_sticker: None,
            conversation_history: None,
        }
    }
    /// Text-only delivery through NyxID's generic relay reply: no native
    /// controls or message updates, so choices degrade to numbered text.
    pub fn plain_text() -> Self {
        Self {
            edit: false,
            typing: false,
            forms: None,
            buttons: "numbered_text".into(),
            cards: "text".into(),
            attachments: "link_text".into(),
            max_text_chars: 4000,
            inbound_media: None,
            native_media: None,
            native_media_edits: None,
            whatsapp_templates: None,
            whatsapp_template_components: None,
            message_effects: None,
            thread_history: None,
            topic_lifecycle: None,
            topic_creation: None,
            thread_lifecycle: None,
            native_thread_start: None,
            structured_replies: None,
            reply_with_received_sticker: None,
            conversation_history: None,
        }
    }
    /// Legacy capabilities before ordinary relay text-edit issuance is enabled.
    pub fn relay(platform: Platform) -> Self {
        match platform {
            Platform::Telegram => Self::telegram(),
            _ => Self::plain_text(),
        }
    }
    /// The reader rollout enables bounded ordinary text edits, not native controls.
    pub fn relay_with_text_edits(platform: Platform, enabled: bool) -> Self {
        let mut capabilities = Self::relay(platform);
        if enabled {
            match platform {
                Platform::Lark | Platform::Feishu => capabilities.edit = true,
                Platform::Discord => {
                    capabilities.edit = true;
                    capabilities.max_text_chars = 2000;
                }
                _ => {}
            }
        }
        capabilities
    }
}

impl ToolsState {
    pub(in crate::tools) async fn enrich_conversation_capabilities(
        &self,
        caller: AgentCaller,
        event: &EventRefPayload,
        channel: &ResolvedChannel,
        capabilities: &mut Capabilities,
    ) {
        capabilities.reply_with_received_sticker = self
            .received_sticker_capability(caller, channel, event, true)
            .await;
        capabilities.topic_creation = self.topic_creation_capability(caller, event).await;
        capabilities.topic_lifecycle = self.topic_capabilities(caller, event, channel).await;
        capabilities.thread_lifecycle = self.thread_capabilities(caller, event, channel).await;
        capabilities.conversation_history = self.history_capabilities(caller, event, channel).await;
    }
}
