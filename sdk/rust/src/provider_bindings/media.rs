//! Original binding-event attachment publication and separate supplementary commands.
use super::ProviderBindings;
use crate::{Error, Result, agents::AgentAttachment, http::Request};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The exact source event and immutable attachment slot.
#[derive(Debug, Clone)]
pub struct BindingMediaTarget<'a> {
    /// Original binding.
    pub binding_id: &'a str,
    /// Original transport conversation.
    pub conversation_id: &'a str,
    /// Original event.
    pub event_id: &'a str,
    /// Attachment position in the original event.
    pub attachment_index: u8,
}
/// Frozen upload authority; never substitute a new digest when retrying.
#[derive(Debug, Clone)]
pub struct BindingMediaUpload {
    /// Frozen sanitized ASCII filename.
    pub name: String,
    /// SHA-256 of the original file bytes.
    pub sha256: String,
    /// Original event capability expiry, Unix seconds.
    pub event_expires_at: i64,
    /// Optional Gateway generation, distinct from Manager generation.
    pub gateway_binding_generation: Option<u64>,
    /// Original media type.
    pub media_type: String,
}
/// The immutable artifact slot kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindingMediaKind {
    /// Original file reference.
    Original,
    /// Derived exact audio transcript.
    Transcript,
    /// Derived exact text excerpt.
    TextExcerpt,
}
/// Complete attested artifact identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingMediaArtifact {
    /// Original attachment position.
    pub attachment_index: u8,
    /// Artifact slot kind.
    pub kind: BindingMediaKind,
    /// SHA-256 of the exact derived content or original bytes.
    pub content_sha256: String,
    /// SHA-256 of original bytes.
    pub source_sha256: String,
    /// Original sanitized filename.
    pub name: String,
    /// Original media type.
    pub media_type: String,
    /// Original byte count.
    pub size_bytes: u64,
    /// SHA-256 of complete canonical supplementary input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_sha256: Option<String>,
}
/// Supplementary input bound to an existing original command and upload receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingMediaSupplement {
    /// Optional original Gateway generation; null remains valid.
    pub gateway_binding_generation: Option<u64>,
    /// Original event expiry, Unix seconds.
    pub event_expires_at: i64,
    /// Exact attested artifact.
    pub artifact: BindingMediaArtifact,
    /// Exact derived UTF-8 text; empty for the original artifact kind.
    pub derived_text: String,
}
/// A separate durable response, never a rewrite of the original response.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BindingMediaSupplementReceipt {
    /// Supplement command identity.
    pub command_id: String,
    /// Supplement response identity.
    pub response_id: String,
    /// Snapshot/replay resource.
    pub response_url: String,
    /// Current command status.
    pub status: String,
    /// Fixed admission deadline.
    pub queue_deadline: String,
    /// Server-derived parent and artifact provenance.
    pub metadata: BTreeMap<String, String>,
}
impl ProviderBindings<'_> {
    /// Publish original bytes. Retry exactly this slot and content after an uncertain outcome.
    pub async fn upload_event_attachment(
        &self,
        target: &BindingMediaTarget<'_>,
        params: &BindingMediaUpload,
        content: Vec<u8>,
    ) -> Result<AgentAttachment> {
        if !safe_name(&params.name)
            || !hex(&params.sha256)
            || params.event_expires_at <= 0
            || params.gateway_binding_generation == Some(0)
            || content.len() > 20 * 1024 * 1024
            || !valid_media(&params.media_type)
        {
            return Err(Error::validation("invalid original event media upload"));
        }
        let size = content.len() as u64;
        let index = target.attachment_index.to_string();
        let path = segments(target, &index, "attachment");
        let mut query = vec![
            ("name", params.name.clone()),
            ("sha256", params.sha256.clone()),
            ("event_expires_at", params.event_expires_at.to_string()),
        ];
        if let Some(generation) = params.gateway_binding_generation {
            query.push(("gateway_binding_generation", generation.to_string()));
        }
        let mut request = Request::binary_operation(&path, content, &params.media_type)?;
        request.query = &query;
        let receipt: AgentAttachment = self.0.request(request).await?;
        validate_receipt(&receipt)?;
        if receipt.name != params.name
            || receipt.sha256 != params.sha256
            || receipt.size_bytes != size
            || receipt.media_type != params.media_type
        {
            return Err(Error::protocol(
                "binding media receipt differs from original attachment",
            ));
        }
        Ok(receipt)
    }
    /// Read the exact completed receipt without fetching source bytes again.
    pub async fn event_attachment(
        &self,
        target: &BindingMediaTarget<'_>,
    ) -> Result<AgentAttachment> {
        let index = target.attachment_index.to_string();
        let receipt: AgentAttachment = self
            .0
            .get(&segments(target, &index, "attachment"), &[])
            .await?;
        validate_receipt(&receipt)?;
        Ok(receipt)
    }
    /// Admit a separate Append response without interrupting the original turn.
    pub async fn supplement_event_media(
        &self,
        target: &BindingMediaTarget<'_>,
        body: &BindingMediaSupplement,
    ) -> Result<BindingMediaSupplementReceipt> {
        if body.artifact.attachment_index != target.attachment_index
            || body.derived_text.len() > 64 * 1024
        {
            return Err(Error::validation(
                "media supplement index or text exceeds its contract",
            ));
        }
        let index = target.attachment_index.to_string();
        self.0
            .operation(
                Method::POST,
                &segments(target, &index, "supplement"),
                Some(body),
                StatusCode::ACCEPTED,
            )
            .await
    }
}
fn segments<'a>(
    target: &'a BindingMediaTarget<'a>,
    index: &'a str,
    suffix: &'a str,
) -> [&'a str; 10] {
    [
        "cmaeg",
        "bindings",
        target.binding_id,
        "conversations",
        target.conversation_id,
        "events",
        target.event_id,
        "media",
        index,
        suffix,
    ]
}
fn validate_receipt(value: &AgentAttachment) -> Result<()> {
    if value.agent_id.is_empty()
        || value.agent_id.len() > 128
        || !value
            .agent_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        || value.size_bytes > 20 * 1024 * 1024
        || !value.id.strip_prefix("att_").is_some_and(hex)
        || !hex(&value.sha256)
        || !safe_name(&value.name)
        || value.path
            != format!(
                "/home/attachments/{}/{}/{}",
                value.agent_id, value.id, value.name
            )
    {
        return Err(Error::protocol("invalid binding media attachment receipt"));
    }
    Ok(())
}
fn hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn safe_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 120
        && !value.starts_with('.')
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
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
