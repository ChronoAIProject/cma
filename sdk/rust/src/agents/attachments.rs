//! Owner-supplied files persisted in the Agent environment.
use super::Agents;
use crate::{
    Error, IdempotencyKey, Result,
    http::{Request, receive},
};
use serde::Deserialize;

/// A verified immutable upload receipt and filesystem path.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentAttachment {
    /// Stable upload identity retained for idempotent retries.
    pub id: String,
    /// Agent which owns the uploaded file.
    pub agent_id: String,
    /// Sanitized filesystem basename.
    pub name: String,
    /// Verified absolute path under /home/attachments.
    pub path: String,
    /// Exact uploaded byte count.
    pub size_bytes: u64,
    /// Original type/subtype; empty file types use application/octet-stream.
    pub media_type: String,
    /// Lowercase SHA-256 digest recorded by the runtime.
    pub sha256: String,
}
impl Agents<'_> {
    /// Upload one file of at most 20 MiB without executing it or sending a message.
    /// Retain the same key, name, media type and bytes when retrying an uncertain upload.
    pub async fn upload_attachment(
        &self,
        agent_id: &str,
        name: &str,
        media_type: &str,
        content: Vec<u8>,
        key: &IdempotencyKey,
    ) -> Result<AgentAttachment> {
        let safe = safe_name(name)?;
        if content.len() > 20 * 1024 * 1024 || !valid_media(media_type) {
            return Err(Error::validation(
                "attachment exceeds 20 MiB or has an invalid media type",
            ));
        }
        let size = content.len() as u64;
        let segments = ["agents", agent_id, "attachments"];
        let query = [("name", name.to_owned())];
        let mut request = Request::binary(&segments, content, media_type, key)?;
        request.query = &query;
        let response = receive(&self.client.inner, request).await?;
        let value: AgentAttachment = serde_json::from_slice(&response.body)
            .map_err(|_| Error::protocol("invalid attachment receipt"))?;
        if !value.id.strip_prefix("att_").is_some_and(hex)
            || value.agent_id != agent_id
            || value.name != safe
            || value.path != format!("/home/attachments/{agent_id}/{}/{}", value.id, safe)
            || value.size_bytes != size
            || value.media_type != media_type
            || !hex(&value.sha256)
        {
            return Err(Error::protocol(
                "attachment receipt does not match requested file and Agent",
            ));
        }
        Ok(value)
    }
}
fn hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn safe_name(value: &str) -> Result<String> {
    if value.is_empty()
        || value.len() > 1024
        || matches!(value, "." | "..")
        || value
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\'))
    {
        return Err(Error::validation(
            "attachment name must not contain path separators or controls",
        ));
    }
    let mut safe: String = value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .take(120)
        .collect();
    if safe.starts_with('.') {
        safe.replace_range(..1, "_");
    }
    Ok(safe)
}
fn valid_media(value: &str) -> bool {
    value.len() <= 127
        && value.split_once('/').is_some_and(|(a, b)| {
            !a.is_empty()
                && !b.is_empty()
                && a.bytes()
                    .chain(b.bytes())
                    .all(|c| c.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&c))
        })
}
