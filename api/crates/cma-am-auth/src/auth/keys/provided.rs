use super::{KeyIssuer, KeyIssuerError, SecretValue};

impl KeyIssuer {
    /// Accept an externally managed key without minting or inferring its expiry.
    /// The binding management authority authorizes delivery; the provider still
    /// verifies the key's scope when the runtime uses it.
    pub fn provided(value: &str) -> Result<SecretValue, KeyIssuerError> {
        let suffix = value
            .strip_prefix("nyxid_ag_")
            .or_else(|| value.strip_prefix("nyx_"));
        if !suffix.is_some_and(|s| {
            (16..=128).contains(&s.len())
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        }) {
            return Err(KeyIssuerError::UnexpectedAuthority);
        }
        Ok(SecretValue::new(value.into()))
    }
}
