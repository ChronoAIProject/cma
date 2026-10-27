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
