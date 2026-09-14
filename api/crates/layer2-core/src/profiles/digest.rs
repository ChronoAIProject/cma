use serde::Serialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use super::ProfileError;

/// SHA-256 of canonical JSON with recursively sorted keys and no whitespace.
pub fn content_digest<T: Serialize>(content: &T) -> Result<String, ProfileError> {
    let value = serde_json::to_value(content)
        .map_err(|_| ProfileError::new("content_not_serializable", "content"))?;
    let bytes = serde_json::to_vec(&canonicalize(value))
        .map_err(|_| ProfileError::new("content_not_serializable", "content"))?;
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
