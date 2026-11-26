use std::sync::Arc;
use std::time::Duration;

use futures_util::{StreamExt as _, TryStreamExt as _};
use layer2_core::profiles::{SkillRef, SkillSource};
use layer2_core::sandbox::{ResolvedAgentSkill, Sandbox};
use layer3_adapter::AgentSkill;
use serde::Deserialize;

use super::credential::SandboxCredentialController;

const ORNN_PROXY_PATH: &[&str] = &["api", "v1", "proxy", "s", "ornn-api"];
const ORNN_SKILLS_PATH: &[&str] = &["api", "v1", "skills"];
const MAX_REFERENCES: usize = 32;
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_URL_BYTES: usize = 2_048;
const LOOKUP_CONCURRENCY: usize = 4;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const RESOLUTION_TIMEOUT: Duration = Duration::from_secs(30);

mod authority;
mod http;
use http::{bounded_body, classify_status};
#[cfg(test)]
pub(super) mod test_support;
#[cfg(test)]
mod warmup;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum AgentSkillResolutionError {
    #[error("skill source is temporarily unavailable: {0}")]
    Unavailable(String),
    #[error("skill reference is unavailable: {0}")]
    NotFound(String),
    #[error("skill source returned invalid material: {0}")]
    Invalid(String),
}

/// Resolves profile references to immutable archives before home materialization.
#[async_trait::async_trait]
pub(crate) trait AgentSkillResolver: Send + Sync {
    #[cfg(test)]
    async fn resolve_warm(
        &self,
        _owner: &layer2_core::SubjectId,
        references: &[SkillRef],
    ) -> Result<Vec<ResolvedAgentSkill>, AgentSkillResolutionError> {
        if references.is_empty() {
            Ok(Vec::new())
        } else {
            Err(unavailable("public warm skill source is not composed"))
        }
    }

    #[cfg(test)]
    async fn warm_archives(
        &self,
        _owner: &layer2_core::SubjectId,
        material: &[ResolvedAgentSkill],
    ) -> Result<Vec<layer3_adapter::SkillArchive>, AgentSkillResolutionError> {
        if material.is_empty() {
            Ok(Vec::new())
        } else {
            Err(unavailable("public warm skill source is not composed"))
        }
    }

    async fn resolve(
        &self,
        sandbox: &Sandbox,
        references: &[SkillRef],
    ) -> Result<Vec<ResolvedAgentSkill>, AgentSkillResolutionError>;

    fn runtime_skills(
        &self,
        material: &[ResolvedAgentSkill],
    ) -> Result<Vec<AgentSkill>, AgentSkillResolutionError>;
}

#[cfg(test)]
mod empty;
#[cfg(test)]
pub(crate) use empty::EmptyAgentSkillResolver;

/// Resolves Ornn references through NyxID's service proxy to immutable material.
pub(crate) struct OrnnAgentSkillResolver {
    http: reqwest::Client,
    request_origin: reqwest::Url,
    locator_origin: reqwest::Url,
    credentials: Option<Arc<dyn SandboxCredentialController>>,
    #[cfg(test)]
    warmup_tokens: Option<Arc<dyn super::credential::OwnerTokenSource>>,
}

impl OrnnAgentSkillResolver {
    pub(crate) fn scoped(
        platform_origin: &str,
        credentials: Arc<dyn SandboxCredentialController>,
    ) -> Result<Arc<Self>, String> {
        Self::new(platform_origin, Some(credentials)).map(Arc::new)
    }

    pub(crate) fn anonymous(platform_origin: &str) -> Result<Arc<Self>, String> {
        Self::new(platform_origin, None).map(Arc::new)
    }

    fn new(
        platform_origin: &str,
        credentials: Option<Arc<dyn SandboxCredentialController>>,
    ) -> Result<Self, String> {
        let origin = parse_platform_origin(platform_origin)?;
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| "agent skill HTTP client is not usable".to_string())?;
        Ok(Self {
            http,
            request_origin: origin.clone(),
            locator_origin: origin,
            credentials,
            #[cfg(test)]
            warmup_tokens: None,
        })
    }

    async fn resolve_one(
        &self,
        index: usize,
        reference: &SkillRef,
        bearer: Option<&str>,
        public_only: bool,
    ) -> Result<ResolvedAgentSkill, AgentSkillResolutionError> {
        let version = requested_version(reference, index)?;
        let url = skill_detail_url(&self.request_origin, reference.id.as_str(), version)
            .map_err(|_| invalid(index, "reference cannot be represented by the Ornn API"))?;
        let mut request = self.http.get(url);
        if let Some(bearer) = bearer {
            request = request.bearer_auth(bearer);
        }
        let response = request
            .send()
            .await
            .map_err(|_| unavailable_at(index, "Ornn request failed"))?;
        classify_status(index, response.status())?;
        let body = bounded_body(response, index).await?;
        let envelope: OrnnEnvelope = serde_json::from_slice(&body)
            .map_err(|_| invalid(index, "Ornn response is not valid JSON"))?;
        if envelope.error.is_some() {
            return Err(invalid(index, "Ornn success response contains an error"));
        }
        let material = envelope
            .data
            .ok_or_else(|| invalid(index, "Ornn success response has no data"))?;
        if public_only && material.is_private != Some(false) {
            return Err(invalid(
                index,
                "only explicitly public skills may be prepared for shared warm environments",
            ));
        }
        if material.guid != reference.id.as_str() {
            return Err(invalid(index, "Ornn response changed the skill identity"));
        }
        if !canonical_ornn_version(&material.version)
            || version.is_some_and(|version| {
                canonical_ornn_version(version) && material.version != version
            })
        {
            return Err(invalid(index, "Ornn response changed the pinned version"));
        }
        if !safe_skill_name(&material.name) {
            return Err(invalid(index, "Ornn response has an invalid skill name"));
        }
        if !canonical_sha256(&material.skill_hash) {
            return Err(invalid(index, "Ornn response has an invalid SHA-256"));
        }
        Ok(ResolvedAgentSkill {
            source: reference.source,
            id: reference.id.clone(),
            name: Some(material.name),
            version: material.version,
            archive_sha256: material.skill_hash,
        })
    }
}

#[async_trait::async_trait]
impl AgentSkillResolver for OrnnAgentSkillResolver {
    #[cfg(test)]
    async fn resolve_warm(
        &self,
        owner: &layer2_core::SubjectId,
        references: &[SkillRef],
    ) -> Result<Vec<ResolvedAgentSkill>, AgentSkillResolutionError> {
        self.resolve_public_material(owner, references).await
    }

    #[cfg(test)]
    async fn warm_archives(
        &self,
        owner: &layer2_core::SubjectId,
        material: &[ResolvedAgentSkill],
    ) -> Result<Vec<layer3_adapter::SkillArchive>, AgentSkillResolutionError> {
        self.download_public_material(owner, material).await
    }
    async fn resolve(
        &self,
        sandbox: &Sandbox,
        references: &[SkillRef],
    ) -> Result<Vec<ResolvedAgentSkill>, AgentSkillResolutionError> {
        if references.len() > MAX_REFERENCES {
            return Err(AgentSkillResolutionError::Invalid(format!(
                "a profile may reference at most {MAX_REFERENCES} skills"
            )));
        }
        if references.is_empty() {
            return Ok(Vec::new());
        }
        let mut ids = std::collections::BTreeSet::new();
        for (index, reference) in references.iter().enumerate() {
            requested_version(reference, index)?;
            if !ids.insert(reference.id.as_str()) {
                return Err(invalid(index, "skill id duplicates another home entry"));
            }
        }
        let resolution = async {
            let bearer = self.scoped_bearer(sandbox).await?;
            let bearer = bearer.as_ref().map(|value| value.expose());
            let requests = references
                .iter()
                .enumerate()
                .map(|(index, reference)| self.resolve_one(index, reference, bearer, false))
                .collect::<Vec<_>>();
            futures_util::stream::iter(requests)
                .buffered(LOOKUP_CONCURRENCY)
                .try_collect()
                .await
        };
        tokio::time::timeout(RESOLUTION_TIMEOUT, resolution)
            .await
            .map_err(|_| unavailable("Ornn resolution deadline elapsed"))?
    }

    fn runtime_skills(
        &self,
        material: &[ResolvedAgentSkill],
    ) -> Result<Vec<AgentSkill>, AgentSkillResolutionError> {
        validate_material(material)?;
        material
            .iter()
            .enumerate()
            .map(|(index, skill)| {
                let source_url =
                    skill_download_url(&self.locator_origin, skill.id.as_str(), &skill.version)
                        .map_err(|_| invalid(index, "download locator is not canonical"))?;
                Ok(AgentSkill {
                    id: skill.id.as_str().to_owned(),
                    name: skill.name.clone(),
                    version: Some(skill.version.clone()),
                    archive_sha256: skill.archive_sha256.clone(),
                    source_url: source_url.to_string(),
                })
            })
            .collect()
    }
}

#[derive(Debug, Deserialize)]
struct OrnnEnvelope {
    data: Option<OrnnMaterial>,
    #[serde(default)]
    error: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct OrnnMaterial {
    guid: String,
    name: String,
    version: String,
    #[serde(rename = "skillHash")]
    skill_hash: String,
    #[serde(default, rename = "isPrivate")]
    is_private: Option<bool>,
}

fn requested_version(
    reference: &SkillRef,
    index: usize,
) -> Result<Option<&str>, AgentSkillResolutionError> {
    if !matches!(reference.source, SkillSource::Ornn) {
        return Err(invalid(index, "skill source is unsupported"));
    }
    if !safe_skill_id(reference.id.as_str()) {
        return Err(invalid(index, "Ornn skill id is not a safe runtime name"));
    }
    let Some(version) = reference
        .version
        .as_deref()
        .filter(|version| !version.is_empty())
    else {
        return Ok(None);
    };
    let tag = version.strip_prefix('@').is_some_and(|tag| {
        !tag.is_empty()
            && tag.len() <= 50
            && tag.as_bytes()[0].is_ascii_lowercase()
            && tag
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    });
    if !canonical_ornn_version(version) && !tag {
        return Err(invalid(
            index,
            "Ornn skill version is neither a literal release nor a supported dist-tag",
        ));
    }
    Ok(Some(version))
}

pub(crate) fn validate_skill_selection(
    references: &[SkillRef],
    material: &[ResolvedAgentSkill],
) -> Result<(), AgentSkillResolutionError> {
    validate_material(material)?;
    if references.len() != material.len() {
        return Err(invalid(0, "skill selection changed the reference count"));
    }
    for (index, (reference, skill)) in references.iter().zip(material).enumerate() {
        let requested = requested_version(reference, index)?;
        if reference.source != skill.source
            || reference.id != skill.id
            || requested
                .is_some_and(|version| canonical_ornn_version(version) && version != skill.version)
        {
            return Err(invalid(
                index,
                "skill selection changed the reference identity or literal pin",
            ));
        }
    }
    Ok(())
}

fn validate_material(material: &[ResolvedAgentSkill]) -> Result<(), AgentSkillResolutionError> {
    if material.len() > MAX_REFERENCES {
        return Err(invalid(0, "skill selection exceeds the reference limit"));
    }
    let mut ids = std::collections::BTreeSet::new();
    for (index, skill) in material.iter().enumerate() {
        if !matches!(skill.source, SkillSource::Ornn)
            || !safe_skill_id(skill.id.as_str())
            || skill
                .name
                .as_deref()
                .is_some_and(|name| !safe_skill_name(name))
            || !ids.insert(skill.id.as_str())
            || !canonical_ornn_version(&skill.version)
            || !canonical_sha256(&skill.archive_sha256)
        {
            return Err(invalid(index, "persisted skill selection is invalid"));
        }
    }
    Ok(())
}

fn safe_skill_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && !id.starts_with('.')
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn canonical_ornn_version(version: &str) -> bool {
    version.len() <= 20
        && version
            .split_once('.')
            .is_some_and(|(major, minor)| canonical_decimal(major) && canonical_decimal(minor))
}

fn safe_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 256
        && name.trim() == name
        && !name.chars().any(char::is_control)
}

fn canonical_decimal(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && (value == "0" || !value.starts_with('0'))
}

fn canonical_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn skill_detail_url(
    origin: &reqwest::Url,
    id: &str,
    version: Option<&str>,
) -> Result<reqwest::Url, ()> {
    let mut url = skill_url(origin, id)?;
    if let Some(version) = version {
        url.query_pairs_mut().append_pair("version", version);
    }
    valid_url(&url).then_some(url).ok_or(())
}

fn skill_download_url(origin: &reqwest::Url, id: &str, version: &str) -> Result<reqwest::Url, ()> {
    let mut url = skill_url(origin, id)?;
    url.path_segments_mut()
        .map_err(|_| ())?
        .extend(["versions", version, "download"]);
    valid_url(&url).then_some(url).ok_or(())
}

fn skill_url(origin: &reqwest::Url, id: &str) -> Result<reqwest::Url, ()> {
    if !safe_skill_id(id) {
        return Err(());
    }
    let mut url = origin.clone();
    url.path_segments_mut()
        .map_err(|_| ())?
        .extend(ORNN_PROXY_PATH.iter().chain(ORNN_SKILLS_PATH).copied())
        .push(id);
    Ok(url)
}

fn valid_url(url: &reqwest::Url) -> bool {
    url.as_str().len() <= MAX_URL_BYTES
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
}

fn parse_platform_origin(raw: &str) -> Result<reqwest::Url, String> {
    let parsed = reqwest::Url::parse(raw)
        .map_err(|_| "agent skill platform origin is not a valid URL".to_string())?;
    let canonical = parsed.origin().ascii_serialization();
    if raw.len() > MAX_URL_BYTES
        || parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.path() != "/"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || raw != canonical
    {
        return Err("agent skill platform origin must be one canonical HTTPS origin".into());
    }
    Ok(parsed)
}

fn unavailable(detail: &str) -> AgentSkillResolutionError {
    AgentSkillResolutionError::Unavailable(detail.into())
}

fn unavailable_at(index: usize, detail: &str) -> AgentSkillResolutionError {
    AgentSkillResolutionError::Unavailable(format!("skill reference {index}: {detail}"))
}

fn invalid(index: usize, detail: &str) -> AgentSkillResolutionError {
    AgentSkillResolutionError::Invalid(format!("skill reference {index}: {detail}"))
}

#[cfg(test)]
#[path = "agent_skills_tests.rs"]
mod tests;
