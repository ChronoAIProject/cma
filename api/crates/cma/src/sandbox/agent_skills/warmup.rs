//! Fetch public Profile assets in the control plane; never send its bearer to a runtime.

use super::*;
use crate::auth::persistence::SecretValue;
use crate::sandbox::credential::OwnerTokenSource;
use layer2_core::SubjectId;

impl OrnnAgentSkillResolver {
    pub(crate) fn with_warmup_tokens(
        self: Arc<Self>,
        tokens: Arc<dyn OwnerTokenSource>,
    ) -> Arc<Self> {
        Arc::new(Self {
            http: self.http.clone(),
            request_origin: self.request_origin.clone(),
            locator_origin: self.locator_origin.clone(),
            credentials: self.credentials.clone(),
            warmup_tokens: Some(tokens),
        })
    }

    async fn publisher_token(
        &self,
        owner: &SubjectId,
    ) -> Result<SecretValue, AgentSkillResolutionError> {
        self.warmup_tokens
            .as_ref()
            .ok_or_else(|| unavailable("Profile publisher access is not composed"))?
            .token_for(owner)
            .await
            .map_err(|_| unavailable("Profile publisher access is unavailable"))?
            .ok_or_else(|| unavailable("Profile publisher access requires renewal"))
    }

    pub(super) async fn resolve_public_material(
        &self,
        owner: &SubjectId,
        references: &[SkillRef],
    ) -> Result<Vec<ResolvedAgentSkill>, AgentSkillResolutionError> {
        if references.is_empty() {
            return Ok(Vec::new());
        }
        if references.len() > MAX_REFERENCES {
            return Err(invalid(0, "too many public skills"));
        }
        let token = self.publisher_token(owner).await?;
        let requests = references
            .iter()
            .enumerate()
            .map(|(index, reference)| {
                self.resolve_one(index, reference, Some(token.expose()), true)
            })
            .collect::<Vec<_>>();
        tokio::time::timeout(
            RESOLUTION_TIMEOUT,
            futures_util::stream::iter(requests)
                .buffered(LOOKUP_CONCURRENCY)
                .try_collect(),
        )
        .await
        .map_err(|_| unavailable("public skill resolution deadline elapsed"))?
    }

    pub(super) async fn download_public_material(
        &self,
        owner: &SubjectId,
        material: &[ResolvedAgentSkill],
    ) -> Result<Vec<layer3_adapter::SkillArchive>, AgentSkillResolutionError> {
        if material.is_empty() {
            return Ok(Vec::new());
        }
        validate_material(material)?;
        let token = self.publisher_token(owner).await?;
        let mut archives = Vec::with_capacity(material.len());
        let mut total_bytes = 0_usize;
        for (index, skill) in material.iter().enumerate() {
            let reference = SkillRef {
                source: skill.source,
                id: skill.id.clone(),
                version: Some(skill.version.clone()),
            };
            if self
                .resolve_one(index, &reference, Some(token.expose()), true)
                .await?
                != *skill
            {
                return Err(invalid(
                    index,
                    "public skill material changed after reservation",
                ));
            }
            let url = skill_download_url(&self.request_origin, skill.id.as_str(), &skill.version)
                .map_err(|_| invalid(index, "public skill download locator is invalid"))?;
            let response = self
                .http
                .get(url)
                .bearer_auth(token.expose())
                .send()
                .await
                .map_err(|_| unavailable_at(index, "public skill download failed"))?;
            classify_status(index, response.status())?;
            let mut bytes = Vec::new();
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk
                    .map_err(|_| unavailable_at(index, "public skill download interrupted"))?;
                if bytes.len().saturating_add(chunk.len()) > 64 * 1024 * 1024 {
                    return Err(invalid(index, "public skill archive is too large"));
                }
                total_bytes = total_bytes.saturating_add(chunk.len());
                if total_bytes > 128 * 1024 * 1024 {
                    return Err(invalid(
                        index,
                        "combined public skill archives are too large",
                    ));
                }
                bytes.extend_from_slice(&chunk);
            }
            let archive = layer3_adapter::SkillArchive {
                sha256: skill.archive_sha256.clone(),
                bytes,
            };
            archive
                .validate()
                .map_err(|_| invalid(index, "public skill archive checksum changed"))?;
            archives.push(archive);
        }
        Ok(archives)
    }
}
