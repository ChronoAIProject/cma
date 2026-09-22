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
