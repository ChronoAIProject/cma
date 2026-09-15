//! Explicitly published immutable content scoped to one retained Agent.
use super::Agents;
use crate::{
    Error, IdempotencyKey, PageOptions, ResourcePage, Result,
    http::{Request, receive},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use reqwest::{Method, StatusCode, header};
use serde::{Deserialize, Serialize};

const MAX_BYTES: usize = 1024 * 1024;

/// Explicit content selected for publication to this Agent's current and future readers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishAgentArtifactRequest {
    /// Display name only, at most 160 UTF-8 bytes without controls or path separators.
    pub name: String,
    /// Supported inert file media type; downloads always use attachment disposition.
    pub media_type: String,
    /// Canonical standard base64 for 1 byte through 1 MiB of selected content.
    pub content_base64: String,
}

/// Immutable approved artifact metadata; no filesystem path or download credential.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "WireArtifact")]
pub struct AgentArtifactMetadata {
    /// Stable artifact identity.
    pub id: String,
    /// Exact retained Agent identity.
    pub agent_id: String,
    /// Safe display name selected by the publisher.
    pub name: String,
    /// Declared content media type.
    pub media_type: String,
    /// Exact decoded byte count, at most 1 MiB.
    pub size_bytes: u64,
    /// Lowercase SHA-256 digest of the retained content.
    pub sha256: String,
    /// Original publication timestamp.
    pub created_at: String,
}

#[derive(Deserialize)]
struct WireArtifact {
    id: String,
    agent_id: String,
    name: String,
    media_type: String,
    size_bytes: u64,
    sha256: String,
    created_at: String,
}
fn hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
fn valid_id(value: &str) -> bool {
    value.strip_prefix("art_").is_some_and(hex)
}
fn valid_name_media(name: &str, media: &str) -> bool {
    !name.is_empty()
        && name.len() <= 160
        && name.trim() == name
        && !matches!(name, "." | "..")
        && !name
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\'))
        && matches!(
            media,
            "text/plain"
                | "text/markdown"
                | "application/json"
                | "application/pdf"
                | "application/octet-stream"
                | "image/png"
                | "image/jpeg"
                | "image/webp"
        )
}
impl TryFrom<WireArtifact> for AgentArtifactMetadata {
    type Error = String;
    fn try_from(value: WireArtifact) -> std::result::Result<Self, Self::Error> {
        if !valid_id(&value.id)
            || value.agent_id.is_empty()
            || !valid_name_media(&value.name, &value.media_type)
            || value.size_bytes == 0
            || value.size_bytes > MAX_BYTES as u64
            || !hex(&value.sha256)
            || value.created_at.is_empty()
        {
            return Err("invalid immutable artifact metadata".into());
        }
        Ok(Self {
            id: value.id,
            agent_id: value.agent_id,
            name: value.name,
            media_type: value.media_type,
            size_bytes: value.size_bytes,
            sha256: value.sha256,
            created_at: value.created_at,
        })
    }
}
impl AgentArtifactMetadata {
    fn require_identity(&self, agent_id: &str, id: Option<&str>) -> Result<()> {
        if self.agent_id != agent_id || id.is_some_and(|id| id != self.id) {
            return Err(Error::protocol(
                "artifact identity did not match the request",
            ));
        }
        Ok(())
    }
}
impl Agents<'_> {
    /// List approved artifacts with fresh current authority and ordered pagination.
    pub async fn list_artifacts(
        &self,
        agent_id: &str,
        options: &PageOptions,
    ) -> Result<ResourcePage<AgentArtifactMetadata>> {
        if options.limit.is_some_and(|limit| limit == 0 || limit > 50)
            || options
                .cursor
                .as_ref()
                .is_some_and(|cursor| !valid_id(cursor))
        {
            return Err(Error::validation(
                "artifact list requires limit 1 to 50 and a server-issued cursor",
            ));
        }
        let page: ResourcePage<AgentArtifactMetadata> = self
            .client
            .get(&["agents", agent_id, "artifacts"], &options.query())
            .await?;
        if page.items.len() > usize::from(options.limit.unwrap_or(50)) {
            return Err(Error::protocol("artifact page exceeded its limit"));
        }
        let mut previous = options.cursor.as_deref();
        for item in &page.items {
            item.require_identity(agent_id, None)?;
            if previous.is_some_and(|id| id >= item.id.as_str()) {
                return Err(Error::protocol("artifact page order did not advance"));
            }
            previous = Some(&item.id);
        }
        if page
            .next
            .as_deref()
            .is_some_and(|next| Some(next) != previous || page.items.is_empty())
        {
            return Err(Error::protocol(
                "artifact continuation did not match the page",
            ));
        }
        Ok(page)
    }
    /// Read one approved artifact's immutable metadata under current Agent access.
    pub async fn read_artifact(&self, agent_id: &str, id: &str) -> Result<AgentArtifactMetadata> {
        require_id(id)?;
        let value: AgentArtifactMetadata = self
            .client
            .get(&["agents", agent_id, "artifacts", id], &[])
            .await?;
        value.require_identity(agent_id, Some(id))?;
        Ok(value)
    }
    /// Publish selected content once with a retained key and current sharing-management authority.
    pub async fn publish_artifact(
        &self,
        agent_id: &str,
        body: &PublishAgentArtifactRequest,
        key: &IdempotencyKey,
    ) -> Result<AgentArtifactMetadata> {
        if !valid_name_media(&body.name, &body.media_type)
            || body.content_base64.len() > MAX_BYTES.div_ceil(3) * 4
        {
            return Err(Error::validation(
                "invalid artifact display name, media type or content size",
            ));
        }
        let content = STANDARD
            .decode(&body.content_base64)
            .map_err(|_| Error::validation("artifact content must be canonical standard base64"))?;
        if content.is_empty()
            || content.len() > MAX_BYTES
            || STANDARD.encode(&content) != body.content_base64
        {
            return Err(Error::validation(
                "artifact content must contain 1 byte through 1 MiB",
            ));
        }
        let value: AgentArtifactMetadata = self
            .client
            .command(
                Method::POST,
                &["agents", agent_id, "artifacts"],
                Some(body),
                key,
                StatusCode::CREATED,
            )
            .await?;
        value.require_identity(agent_id, None)?;
        if value.name != body.name
            || value.media_type != body.media_type
            || value.size_bytes != content.len() as u64
        {
            return Err(Error::protocol(
                "published artifact did not match the request",
            ));
        }
        Ok(value)
    }
    /// Retrieve selected artifact bytes. This does not render or execute their content.
    pub async fn download_artifact(&self, agent_id: &str, id: &str) -> Result<Vec<u8>> {
        require_id(id)?;
        let response = receive(
            &self.client.inner,
            Request::get(&["agents", agent_id, "artifacts", id, "download"]),
        )
        .await?;
        let media = response
            .headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .map(str::trim);
        if !media.is_some_and(|media| media.eq_ignore_ascii_case("application/octet-stream"))
            || response.body.is_empty()
            || response.body.len() > MAX_BYTES
        {
            return Err(Error::protocol(
                "artifact download did not return bounded opaque bytes",
            ));
        }
        Ok(response.body)
    }
}
fn require_id(id: &str) -> Result<()> {
    if valid_id(id) {
        Ok(())
    } else {
        Err(Error::validation("invalid artifact identity"))
    }
}
