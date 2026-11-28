use super::KeyIssuerError;

const KEY_NAME_PREFIX: &str = "cma";

pub(super) fn validate_credential_id(value: &str) -> Result<(), KeyIssuerError> {
    if value.is_empty() || value.len() > 128 || !valid_identifier_bytes(value) {
        Err(KeyIssuerError::InvalidCredentialId)
    } else {
        Ok(())
    }
}

pub fn valid_provider_key_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && valid_identifier_bytes(value)
}

pub(super) fn key_name(deployment: &str, credential_id: &str) -> Result<String, KeyIssuerError> {
    validate_credential_id(deployment)?;
    validate_credential_id(credential_id)?;
    Ok(format!("{KEY_NAME_PREFIX}:{deployment}:{credential_id}"))
}

pub(super) fn credential_of(deployment: &str, name: &str) -> Option<String> {
    let prefix = format!("{KEY_NAME_PREFIX}:{deployment}:");
    let credential_id = name.strip_prefix(&prefix)?;
    validate_credential_id(credential_id)
        .is_ok()
        .then(|| credential_id.to_owned())
}

fn valid_identifier_bytes(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}
