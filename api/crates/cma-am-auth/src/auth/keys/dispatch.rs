//! Separate retryable grant preparation from the consuming credential POST.
use super::*;

pub struct ResolvedMintService {
    pub id: String,
    pub slug: String,
    pub catalog_id: Option<String>,
}

/// No Clone implementation: only the originating preparation can consume this
/// request. A persisted intent never reconstructs a dispatch capability.
pub struct PreparedMint {
    request: reqwest::RequestBuilder,
    allowed_services: Vec<String>,
    allowed_nodes: Vec<String>,
    expires_at: SystemTime,
    request_digest: String,
    provider_identity: String,
}

#[derive(Debug)]
pub struct MintDispatchFailure {
    pub phase: &'static str,
    pub http_status: Option<u16>,
    pub error: KeyIssuerError,
}

impl MintDispatchFailure {
    fn new(phase: &'static str, http_status: Option<u16>, error: KeyIssuerError) -> Self {
        Self {
            phase,
            http_status,
            error,
        }
    }
}

impl PreparedMint {
    pub fn request_digest(&self) -> &str {
        &self.request_digest
    }
    pub fn provider_identity(&self) -> &str {
        &self.provider_identity
    }

    pub async fn dispatch(self) -> Result<MintedKey, MintDispatchFailure> {
        // Renewal owns the local window; preparation and intent persistence
        // cannot reserve a fresh window or recover an expired permit.
        let missing_authority = |_| {
            MintDispatchFailure::new("work_authority", None, KeyIssuerError::UnexpectedAuthority)
        };
        let authorities =
            layer2_core::sandbox::store::WorkAuthority::current().map_err(missing_authority)?;
        let deadline = authorities
            .iter()
            .map(|authority| authority.deadline())
            .collect::<Result<Vec<_>, _>>()
            .map_err(missing_authority)?
            .into_iter()
            .min();
        let request = match deadline {
            Some(deadline) => self
                .request
                .timeout(deadline.saturating_duration_since(tokio::time::Instant::now())),
            None => self.request,
        };
        let response = request.send().await.map_err(|error| {
            MintDispatchFailure::new("send", None, KeyIssuerError::Unavailable(error.to_string()))
        })?;
        let status = response.status();
        if !status.is_success() {
            return Err(MintDispatchFailure::new(
                "response_status",
                Some(status.as_u16()),
                KeyIssuerError::Refused(status.as_u16()),
            ));
        }
        let response = response
            .json::<MintedKeyResponse>()
            .await
            .map_err(|error| {
                MintDispatchFailure::new(
                    "response_decode",
                    Some(status.as_u16()),
                    KeyIssuerError::Unavailable(error.to_string()),
                )
            })?;
        minted_key_from_response(
            response,
            &self.allowed_services,
            &self.allowed_nodes,
            self.expires_at,
        )
        .ok_or_else(|| {
            MintDispatchFailure::new(
                "response_authority",
                Some(status.as_u16()),
                KeyIssuerError::UnexpectedAuthority,
            )
        })
    }
}

impl KeyIssuer {
    pub fn provider_identity(&self) -> String {
        format!(
            "{:x}",
            Sha256::digest(format!("{}\0{}", self.base_url, self.deployment))
        )
    }

    pub async fn prepare_mint(
        &self,
        credential_id: &str,
        owner: &layer2_core::SubjectId,
        owner_token: &str,
        services: &[ServiceId],
    ) -> Result<PreparedMint, KeyIssuerError> {
        validate_credential_id(credential_id)?;
        let allowed = self.resolve_exact(owner_token, services).await?;
        self.prepare_allowed_mint(credential_id, owner, owner_token, allowed)
            .await
    }

    /// Resolve the other services alongside an already verified connection.
    /// The composing caller confirms that the provider rows form a valid runtime route.
    pub async fn prepare_mint_with_connection<T>(
        &self,
        credential_id: &str,
        owner: &layer2_core::SubjectId,
        owner_token: &str,
        services: &[ServiceId],
        selected: (&ServiceId, &str),
        route: impl FnOnce(Vec<ResolvedMintService>) -> Result<T, KeyIssuerError>,
    ) -> Result<(PreparedMint, T), KeyIssuerError> {
        let (selected_service, selected_id) = selected;
        if !services.contains(selected_service) {
            return Err(KeyIssuerError::UnexpectedAuthority);
        }
        let others = services
            .iter()
            .filter(|service| *service != selected_service)
            .cloned()
            .collect::<Vec<_>>();
        let mut resolved = Vec::new();
        if !others.is_empty() {
            let rows = self.service_rows(USER_SERVICES_PATH, owner_token).await?;
            for row in Self::resolve_service_rows(&rows, &others)? {
                resolved.push(ResolvedMintService {
                    id: row.id.clone(),
                    slug: row
                        .slug
                        .clone()
                        .ok_or(KeyIssuerError::UnexpectedAuthority)?,
                    catalog_id: row.catalog_service_id.clone(),
                });
            }
        }
        let mut allowed = resolved
            .iter()
            .map(|row| row.id.clone())
            .collect::<Vec<_>>();
        allowed.push(selected_id.to_owned());
        let route = route(resolved)?;
        let prepared = self
            .prepare_allowed_mint(credential_id, owner, owner_token, allowed)
            .await?;
        Ok((prepared, route))
    }

    async fn prepare_allowed_mint(
        &self,
        credential_id: &str,
        owner: &layer2_core::SubjectId,
        owner_token: &str,
        allowed: Vec<String>,
    ) -> Result<PreparedMint, KeyIssuerError> {
        validate_credential_id(credential_id)?;
        let plan = self
            .scope_plan(
                owner_token,
                &allowed,
                &layer2_core::resource_access::ResourceOwner::Person {
                    subject: owner.clone(),
                },
            )
            .await?;
        let expires_at = key_expiry(SystemTime::now());
        let mut body = mint_request_body(&self.deployment, credential_id, &allowed, expires_at)?;
        plan.apply(&mut body);
        let encoded = serde_json::to_vec(&body).map_err(|_| KeyIssuerError::UnexpectedAuthority)?;
        Ok(PreparedMint {
            request: self
                .http
                .post(format!("{}{KEYS_PATH}", self.base_url))
                .bearer_auth(owner_token)
                .json(&body),
            allowed_services: allowed,
            allowed_nodes: plan.allowed_node_ids,
            expires_at,
            request_digest: format!("{:x}", Sha256::digest(encoded)),
            provider_identity: format!(
                "{:x}",
                Sha256::digest(format!("{}\0{}", self.base_url, self.deployment))
            ),
        })
    }
}
