use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use super::{TriggerContent, TriggerError};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerRevision {
    pub revision: u32,
    pub content: TriggerContent,
    pub digest: String,
    pub published_at: String,
}

impl TriggerRevision {
    pub fn new(
        revision: u32,
        content: TriggerContent,
        published_at: String,
    ) -> Result<Self, TriggerError> {
        content.validate()?;
        if revision == 0 {
            return Err(TriggerError::new("invalid_trigger_revision", "revision"));
        }
        if published_at.trim().is_empty() {
            return Err(TriggerError::new("invalid_timestamp", "published_at"));
        }
        Ok(Self {
            revision,
            digest: trigger_content_digest(&content)?,
            content,
            published_at,
        })
    }

    pub fn validate(&self) -> Result<(), TriggerError> {
        self.content.validate()?;
        if self.revision == 0 {
            return Err(TriggerError::new("invalid_trigger_revision", "revision"));
        }
        if self.published_at.trim().is_empty() {
            return Err(TriggerError::new("invalid_timestamp", "published_at"));
        }
        if self.digest != trigger_content_digest(&self.content)? {
            return Err(TriggerError::new("trigger_digest_mismatch", "digest"));
        }
        Ok(())
    }
}

/// SHA-256 of canonical JSON with recursively sorted object keys and no whitespace.
pub fn trigger_content_digest(content: &TriggerContent) -> Result<String, TriggerError> {
    let value = serde_json::to_value(content)
        .map_err(|_| TriggerError::new("content_not_serializable", "content"))?;
    let bytes = serde_json::to_vec(&canonicalize(value))
        .map_err(|_| TriggerError::new("content_not_serializable", "content"))?;
    let digest = Sha256::digest(bytes);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn canonicalize(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(canonicalize).collect()),
        Value::Object(values) => {
            let mut entries: Vec<_> = values.into_iter().collect();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, canonicalize(value)))
                    .collect::<Map<_, _>>(),
            )
        }
        scalar => scalar,
    }
}
