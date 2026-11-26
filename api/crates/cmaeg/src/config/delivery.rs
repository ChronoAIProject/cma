use super::{positive_duration, ConfigError};
use cmaeg_core::channel::Platform;
use std::{env, time::Duration};
pub const CMAEG_NYXID_REPLY_TIMEOUT_MS: &str = "CMAEG_NYXID_REPLY_TIMEOUT_MS";
pub const CMAEG_NYXID_COMMONMARK_RELAY_ENABLED: &str = "CMAEG_NYXID_COMMONMARK_RELAY_ENABLED";
pub(crate) const CMAEG_NYXID_COMMONMARK_LARK_FEISHU_DISCORD_ENABLED: &str =
    "CMAEG_NYXID_COMMONMARK_LARK_FEISHU_DISCORD_ENABLED";
pub const CMAEG_NYXID_COMPLETE_REPLIES: &str = "CMAEG_NYXID_COMPLETE_REPLIES";
pub const CMAEG_NYXID_NATIVE_MEDIA_ENABLED: &str = "CMAEG_NYXID_NATIVE_MEDIA_ENABLED";
pub const CMAEG_RELAY_TEXT_EDITS_ENABLED: &str = "CMAEG_RELAY_TEXT_EDITS_ENABLED";
pub const CMAEG_NATIVE_DISCORD_EDITS_ENABLED: &str = "CMAEG_NATIVE_DISCORD_EDITS_ENABLED";
pub const CMAEG_NATIVE_DISCORD_MESSAGE_EFFECTS_ENABLED: &str =
    "CMAEG_NATIVE_DISCORD_MESSAGE_EFFECTS_ENABLED";
pub const CMAEG_NATIVE_DISCORD_MEDIA_EFFECTS_ENABLED: &str =
    "CMAEG_NATIVE_DISCORD_MEDIA_EFFECTS_ENABLED";
pub const CMAEG_NATIVE_DISCORD_THREAD_METADATA_ENABLED: &str =
    "CMAEG_NATIVE_DISCORD_THREAD_METADATA_ENABLED";
pub const CMAEG_CONSUMER_INTERACTIONS_ENABLED: &str = "CMAEG_CONSUMER_INTERACTIONS_ENABLED";
pub const CMAEG_WHATSAPP_INTERACTIONS_ENABLED: &str = "CMAEG_WHATSAPP_INTERACTIONS_ENABLED";
pub const CMAEG_DISCORD_INTERACTIONS_ENABLED: &str = "CMAEG_DISCORD_INTERACTIONS_ENABLED";
pub const CMAEG_DISCORD_CARD_EFFECTS_ENABLED: &str = "CMAEG_DISCORD_CARD_EFFECTS_ENABLED";
pub const CMAEG_DISCORD_CARD_EDITS_ENABLED: &str = "CMAEG_DISCORD_CARD_EDITS_ENABLED";
pub const CMAEG_DISCORD_MULTISELECT_ENABLED: &str = "CMAEG_DISCORD_MULTISELECT_ENABLED";
pub const CMAEG_WHATSAPP_TEMPLATES_ENABLED: &str = "CMAEG_WHATSAPP_TEMPLATES_ENABLED";
pub const CMAEG_LARK_CARDS_ENABLED: &str = "CMAEG_LARK_CARDS_ENABLED";
pub const CMAEG_DISCORD_MESSAGE_EVENTS_ENABLED: &str = "CMAEG_DISCORD_MESSAGE_EVENTS_ENABLED";
pub const CMAEG_DISCORD_FORMS_ENABLED: &str = "CMAEG_DISCORD_FORMS_ENABLED";
pub const CMAEG_LARK_FORMS_ENABLED: &str = "CMAEG_LARK_FORMS_ENABLED";
pub const CMAEG_TELEGRAM_MESSAGE_EFFECTS_ENABLED: &str = "CMAEG_TELEGRAM_MESSAGE_EFFECTS_ENABLED";
pub const CMAEG_TELEGRAM_MEDIA_EFFECTS_ENABLED: &str = "CMAEG_TELEGRAM_MEDIA_EFFECTS_ENABLED";
pub const CMAEG_ACK_TEXT: &str = "CMAEG_ACK_TEXT";
pub const CMAEG_HEARTBEAT_MS: &str = "CMAEG_HEARTBEAT_MS";
pub const CMAEG_FAILURE_TEXT: &str = "CMAEG_FAILURE_TEXT";
#[derive(Debug, Clone)]
pub struct DeliveryConfig {
    pub reply_timeout: Duration,
    pub ack_text: String,
    pub heartbeat: Duration,
    pub failure_text: String,
}
impl Default for DeliveryConfig {
    fn default() -> Self {
        Self {
            reply_timeout: Duration::from_secs(5),
            ack_text: "Working on it…".into(),
            heartbeat: Duration::from_secs(60),
            failure_text: "Sorry, I couldn’t finish that request. Please try again.".into(),
        }
    }
}
impl DeliveryConfig {
    pub(super) fn from_env() -> Result<Self, ConfigError> {
        let defaults = Self::default();
        Ok(Self {
            reply_timeout: positive_duration(CMAEG_NYXID_REPLY_TIMEOUT_MS, 5000)?,
            heartbeat: positive_duration(CMAEG_HEARTBEAT_MS, 60000)?,
            ack_text: text(CMAEG_ACK_TEXT, defaults.ack_text)?,
            failure_text: text(CMAEG_FAILURE_TEXT, defaults.failure_text)?,
        })
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct CommonmarkRelay {
    pub telegram: bool,
    pub lark_feishu_discord: bool,
}

impl CommonmarkRelay {
    pub(crate) fn from_env() -> Result<Self, ConfigError> {
        Ok(Self {
            telegram: super::routing::write_flag(CMAEG_NYXID_COMMONMARK_RELAY_ENABLED)?,
            lark_feishu_discord: super::routing::write_flag(
                CMAEG_NYXID_COMMONMARK_LARK_FEISHU_DISCORD_ENABLED,
            )?,
        })
    }

    pub(crate) fn enabled_for(self, platform: Platform) -> bool {
        match platform {
            Platform::Telegram => self.telegram,
            Platform::Lark | Platform::Feishu | Platform::Discord => self.lark_feishu_discord,
            Platform::Slack | Platform::Whatsapp => false,
        }
    }
}

pub(crate) fn nyxid_complete_replies_from_env() -> Result<bool, ConfigError> {
    complete_replies(env::var(CMAEG_NYXID_COMPLETE_REPLIES))
}

pub(crate) fn nyxid_native_media_from_env() -> Result<bool, ConfigError> {
    native_media(env::var(CMAEG_NYXID_NATIVE_MEDIA_ENABLED))
}

pub(crate) fn relay_text_edits_from_env() -> Result<bool, ConfigError> {
    relay_text_edits(env::var(CMAEG_RELAY_TEXT_EDITS_ENABLED))
}

pub(crate) fn native_discord_edits_from_env() -> Result<bool, ConfigError> {
    super::routing::write_flag(CMAEG_NATIVE_DISCORD_EDITS_ENABLED)
}
pub(crate) fn native_discord_effects_from_env() -> Result<bool, ConfigError> {
    super::routing::write_flag(CMAEG_NATIVE_DISCORD_MESSAGE_EFFECTS_ENABLED)
}
pub(crate) fn native_discord_media_effects_from_env() -> Result<bool, ConfigError> {
    super::routing::write_flag(CMAEG_NATIVE_DISCORD_MEDIA_EFFECTS_ENABLED)
}
pub(crate) fn native_discord_thread_metadata_from_env() -> Result<bool, ConfigError> {
    super::routing::write_flag(CMAEG_NATIVE_DISCORD_THREAD_METADATA_ENABLED)
}

pub(crate) fn consumer_interactions_from_env() -> Result<bool, ConfigError> {
    consumer_interactions(env::var(CMAEG_CONSUMER_INTERACTIONS_ENABLED))
}
fn consumer_interactions(value: Result<String, env::VarError>) -> Result<bool, ConfigError> {
    match value.as_deref() {
        Ok("true") => Ok(true),
        Ok("false") | Err(env::VarError::NotPresent) => Ok(false),
        _ => Err(ConfigError::InvalidManagement {
            detail: format!("{CMAEG_CONSUMER_INTERACTIONS_ENABLED} must be true or false"),
        }),
    }
}

pub(crate) fn whatsapp_interactions_from_env() -> Result<bool, ConfigError> {
    whatsapp_interactions(env::var(CMAEG_WHATSAPP_INTERACTIONS_ENABLED))
}
pub(crate) fn discord_interactions_from_env() -> Result<bool, ConfigError> {
    super::routing::write_flag(CMAEG_DISCORD_INTERACTIONS_ENABLED)
}
pub(crate) fn discord_multiselect_from_env() -> Result<bool, ConfigError> {
    super::routing::write_flag(CMAEG_DISCORD_MULTISELECT_ENABLED)
}
fn whatsapp_interactions(value: Result<String, env::VarError>) -> Result<bool, ConfigError> {
    match value.as_deref() {
        Ok("true") => Ok(true),
        Ok("false") | Err(env::VarError::NotPresent) => Ok(false),
        _ => Err(ConfigError::InvalidManagement {
            detail: format!("{CMAEG_WHATSAPP_INTERACTIONS_ENABLED} must be true or false"),
        }),
    }
}

pub(crate) fn whatsapp_templates_from_env() -> Result<bool, ConfigError> {
    whatsapp_templates(env::var(CMAEG_WHATSAPP_TEMPLATES_ENABLED))
}
fn whatsapp_templates(value: Result<String, env::VarError>) -> Result<bool, ConfigError> {
    match value.as_deref() {
        Ok("true") => Ok(true),
        Ok("false") | Err(env::VarError::NotPresent) => Ok(false),
        _ => Err(ConfigError::InvalidManagement {
            detail: format!("{CMAEG_WHATSAPP_TEMPLATES_ENABLED} must be true or false"),
        }),
    }
}

pub(crate) fn lark_cards_from_env() -> Result<bool, ConfigError> {
    lark_cards(env::var(CMAEG_LARK_CARDS_ENABLED))
}
pub(crate) fn discord_card_effects_from_env() -> Result<bool, ConfigError> {
    super::routing::write_flag(CMAEG_DISCORD_CARD_EFFECTS_ENABLED)
}
pub(crate) fn discord_card_edits_from_env() -> Result<bool, ConfigError> {
    super::routing::write_flag(CMAEG_DISCORD_CARD_EDITS_ENABLED)
}
pub(crate) fn discord_reactions_from_env() -> Result<bool, ConfigError> {
    super::routing::write_flag("CMAEG_DISCORD_REACTIONS_ENABLED")
}
pub(crate) fn discord_message_events_from_env() -> Result<bool, ConfigError> {
    super::routing::write_flag(CMAEG_DISCORD_MESSAGE_EVENTS_ENABLED)
}
pub(crate) fn discord_forms_from_env() -> Result<bool, ConfigError> {
    super::routing::write_flag(CMAEG_DISCORD_FORMS_ENABLED)
}
pub(crate) fn lark_forms_from_env() -> Result<bool, ConfigError> {
    super::routing::write_flag(CMAEG_LARK_FORMS_ENABLED)
}
pub(crate) fn telegram_message_effects_from_env() -> Result<bool, ConfigError> {
    telegram_effects(env::var(CMAEG_TELEGRAM_MESSAGE_EFFECTS_ENABLED))
}
pub(crate) fn telegram_media_effects_from_env() -> Result<bool, ConfigError> {
    super::routing::write_flag(CMAEG_TELEGRAM_MEDIA_EFFECTS_ENABLED)
}
fn telegram_effects(value: Result<String, env::VarError>) -> Result<bool, ConfigError> {
    match value.as_deref() {
        Ok("true") => Ok(true),
        Ok("false") | Err(env::VarError::NotPresent) => Ok(false),
        _ => Err(ConfigError::InvalidManagement {
            detail: format!("{CMAEG_TELEGRAM_MESSAGE_EFFECTS_ENABLED} must be true or false"),
        }),
    }
}
fn lark_cards(value: Result<String, env::VarError>) -> Result<bool, ConfigError> {
    match value.as_deref() {
        Ok("true") => Ok(true),
        Ok("false") | Err(env::VarError::NotPresent) => Ok(false),
        _ => Err(ConfigError::InvalidManagement {
            detail: format!("{CMAEG_LARK_CARDS_ENABLED} must be true or false"),
        }),
    }
}

fn relay_text_edits(value: Result<String, env::VarError>) -> Result<bool, ConfigError> {
    match value.as_deref() {
        Ok("true") => Ok(true),
        Ok("false") | Err(env::VarError::NotPresent) => Ok(false),
        _ => Err(ConfigError::InvalidManagement {
            detail: format!("{CMAEG_RELAY_TEXT_EDITS_ENABLED} must be true or false"),
        }),
    }
}

fn native_media(value: Result<String, env::VarError>) -> Result<bool, ConfigError> {
    match value.as_deref() {
        Ok("true") => Ok(true),
        Ok("false") | Err(env::VarError::NotPresent) => Ok(false),
        _ => Err(ConfigError::InvalidManagement {
            detail: format!("{CMAEG_NYXID_NATIVE_MEDIA_ENABLED} must be true or false"),
        }),
    }
}

fn complete_replies(value: Result<String, env::VarError>) -> Result<bool, ConfigError> {
    match value.as_deref() {
        Ok("true") => Ok(true),
        Ok("false") | Err(env::VarError::NotPresent) => Ok(false),
        _ => Err(ConfigError::InvalidManagement {
            detail: format!("{CMAEG_NYXID_COMPLETE_REPLIES} must be true or false"),
        }),
    }
}

fn text(name: &'static str, default: String) -> Result<String, ConfigError> {
    let value = env::var(name).unwrap_or(default);
    if value.trim().is_empty() || value.len() > 1024 || value.chars().any(char::is_control) {
        return Err(ConfigError::InvalidManagement {
            detail: format!("{name} must be nonempty text of at most 1024 bytes without controls"),
        });
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whatsapp_interactions_require_explicit_reader_rollout() {
        assert!(!whatsapp_interactions(Err(env::VarError::NotPresent)).unwrap());
        assert!(!whatsapp_interactions(Ok("false".into())).unwrap());
        assert!(whatsapp_interactions(Ok("true".into())).unwrap());
        for value in ["", "TRUE", "1", " true "] {
            assert!(whatsapp_interactions(Ok(value.into())).is_err());
        }
    }

    #[test]
    fn native_effect_issuance_requires_explicit_reader_rollout() {
        assert!(!telegram_effects(Err(env::VarError::NotPresent)).unwrap());
        assert!(!telegram_effects(Ok("false".into())).unwrap());
        assert!(telegram_effects(Ok("true".into())).unwrap());
        for value in ["", "TRUE", "1", " true "] {
            assert!(telegram_effects(Ok(value.into())).is_err());
        }
    }
    #[test]
    fn templates_require_explicit_relay_attestation() {
        assert!(!whatsapp_templates(Err(env::VarError::NotPresent)).unwrap());
        assert!(!whatsapp_templates(Ok("false".into())).unwrap());
        assert!(whatsapp_templates(Ok("true".into())).unwrap());
        for value in ["", "TRUE", "1", " true "] {
            assert!(whatsapp_templates(Ok(value.into())).is_err());
        }
    }

    #[test]
    fn consumer_interactions_require_explicit_reader_rollout() {
        assert!(!consumer_interactions(Err(env::VarError::NotPresent)).unwrap());
        assert!(!consumer_interactions(Ok("false".into())).unwrap());
        assert!(consumer_interactions(Ok("true".into())).unwrap());
        for value in ["", "TRUE", "1", " true "] {
            assert!(consumer_interactions(Ok(value.into())).is_err());
        }
        assert!(consumer_interactions(Err(env::VarError::NotUnicode("invalid".into()))).is_err());
    }

    #[test]
    fn lark_cards_require_explicit_reader_rollout() {
        assert!(!lark_cards(Err(env::VarError::NotPresent)).unwrap());
        assert!(!lark_cards(Ok("false".into())).unwrap());
        assert!(lark_cards(Ok("true".into())).unwrap());
        for value in ["", "TRUE", "1", " true "] {
            assert!(lark_cards(Ok(value.into())).is_err());
        }
    }

    #[test]
    fn relay_text_edits_require_explicit_reader_rollout_attestation() {
        assert!(!relay_text_edits(Err(env::VarError::NotPresent)).unwrap());
        assert!(!relay_text_edits(Ok("false".into())).unwrap());
        assert!(relay_text_edits(Ok("true".into())).unwrap());
        for value in ["", "TRUE", "1", " true "] {
            assert!(relay_text_edits(Ok(value.into())).is_err());
        }
        assert!(relay_text_edits(Err(env::VarError::NotUnicode("invalid".into()))).is_err());
    }

    #[test]
    fn native_media_requires_explicit_operator_attestation() {
        assert!(!native_media(Err(env::VarError::NotPresent)).unwrap());
        assert!(!native_media(Ok("false".into())).unwrap());
        assert!(native_media(Ok("true".into())).unwrap());
        for value in ["", "TRUE", "1", " true "] {
            assert!(native_media(Ok(value.into())).is_err());
        }
        assert!(native_media(Err(env::VarError::NotUnicode("invalid".into()))).is_err());
    }

    #[test]
    fn complete_replies_requires_explicit_operator_attestation() {
        assert!(!complete_replies(Err(env::VarError::NotPresent)).unwrap());
        assert!(!complete_replies(Ok("false".into())).unwrap());
        assert!(complete_replies(Ok("true".into())).unwrap());
        for value in ["", "TRUE", "1", " true "] {
            assert!(complete_replies(Ok(value.into())).is_err());
        }
        assert!(complete_replies(Err(env::VarError::NotUnicode("invalid".into()))).is_err());
    }
}
