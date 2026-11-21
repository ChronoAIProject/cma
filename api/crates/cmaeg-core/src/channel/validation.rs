//! Channel policy validation helpers.
use super::*;
pub(super) fn validate_admission(
    admission: &AdmissionPolicy,
    pointer: &str,
) -> Result<(), ValidationError> {
    let values = match admission {
        AdmissionPolicy::Allowlist { sender_ids } => Some(sender_ids),
        AdmissionPolicy::Scoped { senders, chats, .. } => {
            for (name, value) in [("senders", senders), ("chats", chats)] {
                if let IdentityAdmission::Allowlist(values) = value {
                    validate_nonempty_unique(values, &format!("{pointer}/admission/{name}"), 512)?;
                }
            }
            None
        }
        AdmissionPolicy::All {} => None,
    };
    if let Some(values) = values {
        validate_nonempty_unique(values, &format!("{pointer}/admission"), 512)?;
    }
    Ok(())
}

/// Per-platform bot identity and trust rules for a NyxID relay source.
pub(super) fn validate_relay_identity(
    platform: Platform,
    bot_username: Option<&str>,
    bot_id: Option<&str>,
    trust_normalized: bool,
    verify_raw_event: bool,
    pointer: &str,
) -> Result<(), ValidationError> {
    if verify_raw_event
        && (trust_normalized || !matches!(platform, Platform::Discord | Platform::Whatsapp))
    {
        return Err(ValidationError::new(
            format!("{pointer}/verify_raw_event"),
            "requires Discord or WhatsApp without trust_normalized",
        ));
    }
    if verify_raw_event
        && platform == Platform::Discord
        && bot_id.is_some_and(|id| {
            id.is_empty() || id.len() > 32 || !id.bytes().all(|byte| byte.is_ascii_digit())
        })
    {
        return Err(ValidationError::new(
            format!("{pointer}/bot_id"),
            "verified Discord requires a numeric receiving bot ID",
        ));
    }
    if let Some(username) = bot_username {
        let (valid, message) = match platform {
            Platform::Telegram => (
                (5..=32).contains(&username.len())
                    && username
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'),
                "must contain 5..=32 Telegram username characters without @",
            ),
            _ => (
                identifier_safe(username, 64),
                "must contain 1..=64 letters, digits, '_', '-' or '.' without @",
            ),
        };
        if !valid {
            return Err(ValidationError::new(
                format!("{pointer}/bot_username"),
                message,
            ));
        }
    }
    if let Some(id) = bot_id {
        let (valid, message) = match platform {
            Platform::Telegram => (
                (1..=20).contains(&id.len()) && id.bytes().all(|byte| byte.is_ascii_digit()),
                "must be the bot's numeric Telegram user ID",
            ),
            // Mentions carry the bot's open_id; app IDs (`cli_`) and union IDs never match.
            Platform::Lark | Platform::Feishu => (
                id.strip_prefix("ou_")
                    .is_some_and(|rest| identifier_safe(rest, 125)),
                "must be the bot's open_id (ou_ followed by at most 125 identifier characters)",
            ),
            Platform::Discord | Platform::Slack | Platform::Whatsapp => (
                identifier_safe(id, 128),
                "must contain 1..=128 letters, digits, '_', '-' or '.'",
            ),
        };
        if !valid {
            return Err(ValidationError::new(format!("{pointer}/bot_id"), message));
        }
    }
    if trust_normalized && platform.verifies_raw_event() {
        return Err(ValidationError::new(
            format!("{pointer}/trust_normalized"),
            "applies only to platforms without a raw-event verifier",
        ));
    }
    Ok(())
}

fn identifier_safe(value: &str, max: usize) -> bool {
    (1..=max).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

pub(super) fn validate_custom_auth(
    auth: &CustomAuth,
    pointer: &str,
) -> Result<(), ValidationError> {
    match auth {
        CustomAuth::Hmac { key_id } => {
            require_string(key_id, &format!("{pointer}/auth/key_id"), 1, 512)
        }
        CustomAuth::NyxidKey { key_hashes } => {
            validate_nonempty_unique(key_hashes, &format!("{pointer}/auth/key_hashes"), 512)?;
            for hash in key_hashes {
                validate_hash(hash, &format!("{pointer}/auth/key_hashes"))?;
            }
            Ok(())
        }
    }
}

pub(super) fn validate_nonempty_unique(
    values: &[String],
    pointer: &str,
    max: usize,
) -> Result<(), ValidationError> {
    if values.is_empty() || values.len() > max {
        return Err(ValidationError::new(
            pointer,
            "must contain a bounded non-empty list",
        ));
    }
    let mut seen = BTreeSet::new();
    for (index, value) in values.iter().enumerate() {
        require_string(value, &format!("{pointer}/{index}"), 1, 512)?;
        if !seen.insert(value) {
            return Err(ValidationError::new(pointer, "values must be unique"));
        }
    }
    Ok(())
}

pub(super) fn validate_hash(value: &str, pointer: &str) -> Result<(), ValidationError> {
    if value.len() != 71
        || !value.starts_with("sha256:")
        || !value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(ValidationError::new(
            pointer,
            "must be sha256: followed by lowercase hex",
        ));
    }
    Ok(())
}

pub(crate) fn validate_https_url(value: &str, pointer: &str) -> Result<(), ValidationError> {
    let url = url::Url::parse(value)
        .map_err(|_| ValidationError::new(pointer, "must be an absolute URL"))?;
    if url.host_str().is_none() || value.chars().any(char::is_whitespace) {
        return Err(ValidationError::new(
            pointer,
            "must include an absolute host",
        ));
    }
    if url.scheme() == "https"
        || (url.scheme() == "http"
            && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]")))
    {
        Ok(())
    } else {
        Err(ValidationError::new(
            pointer,
            "must use https (or local http)",
        ))
    }
}
