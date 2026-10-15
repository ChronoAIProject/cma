use super::{positive_duration, ConfigError};
use std::{env, time::Duration};
pub const CMAEG_NYXID_REPLY_TIMEOUT_MS: &str = "CMAEG_NYXID_REPLY_TIMEOUT_MS";
pub const CMAEG_NYXID_COMPLETE_REPLIES: &str = "CMAEG_NYXID_COMPLETE_REPLIES";
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
pub(crate) fn nyxid_complete_replies_from_env() -> Result<bool, ConfigError> {
    complete_replies(env::var(CMAEG_NYXID_COMPLETE_REPLIES))
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
