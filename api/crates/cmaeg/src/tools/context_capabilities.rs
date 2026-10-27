//! Baseline capabilities projected for each supported delivery transport.
use super::Capabilities;
use cmaeg_core::channel::Platform;

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
            thread_lifecycle: None,
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
            thread_lifecycle: None,
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
            thread_lifecycle: None,
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
