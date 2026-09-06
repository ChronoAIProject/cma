use std::fmt;

use super::required;

#[derive(Clone)]
pub(crate) struct CheckpointStorageConfig {
    pub bucket: String,
    pub client_id: String,
    pub client_secret: String,
}

impl fmt::Debug for CheckpointStorageConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CheckpointStorageConfig")
            .field("bucket", &self.bucket)
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .finish()
    }
}

impl CheckpointStorageConfig {
    pub(super) fn from_lookup(
        lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    ) -> Result<Self, String> {
        let bucket = required(lookup, "CHRONO_CHECKPOINT_BUCKET")?;
        let bytes = bucket.as_bytes();
        if !(3..=63).contains(&bytes.len())
            || !bytes.first().is_some_and(u8::is_ascii_alphanumeric)
            || !bytes.last().is_some_and(u8::is_ascii_alphanumeric)
            || !bytes
                .iter()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
        {
            return Err(
                "CHRONO_CHECKPOINT_BUCKET must be a 3 to 63 character lowercase bucket name".into(),
            );
        }
        let client_id = required(lookup, "CHRONO_STORAGE_CLIENT_ID")?;
        let client_secret = required(lookup, "CHRONO_STORAGE_CLIENT_SECRET")?;
        for (key, value, max) in [
            ("CHRONO_STORAGE_CLIENT_ID", &client_id, 256),
            ("CHRONO_STORAGE_CLIENT_SECRET", &client_secret, 4_096),
        ] {
            if value.len() > max || value.trim() != value || value.chars().any(char::is_control) {
                return Err(format!("{key} must be nonempty, bounded, and contain no surrounding whitespace or control characters"));
            }
        }
        Ok(Self {
            bucket,
            client_id,
            client_secret,
        })
    }
}
