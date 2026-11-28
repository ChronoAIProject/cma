use super::*;
use serde::de::DeserializeOwned;

pub(super) fn unavailable() -> StoreError {
    StoreError::Unavailable("service authorization evidence is unavailable".into())
}

pub(super) async fn read<T: DeserializeOwned>(
    request: reqwest::RequestBuilder,
) -> Result<T, StoreError> {
    let (status, bytes) = response(request).await?;
    if !status.is_success() {
        return Err(unavailable());
    }
    serde_json::from_slice(&bytes).map_err(|_| unavailable())
}

async fn response(
    request: reqwest::RequestBuilder,
) -> Result<(reqwest::StatusCode, Vec<u8>), StoreError> {
    let mut response = request.send().await.map_err(|_| unavailable())?;
    let status = response.status();
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
        if bytes.len() + chunk.len() > 1024 * 1024 {
            return Err(unavailable());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok((status, bytes))
}

pub(super) enum ConsentRead {
    Available(Option<Consent>),
    UnsupportedCredential,
}

pub(super) async fn read_consent(
    request: reqwest::RequestBuilder,
    client: &str,
) -> Result<ConsentRead, StoreError> {
    let (status, bytes) = response(request).await?;
    if status == reqwest::StatusCode::FORBIDDEN {
        let error: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
        if error["error"] == "forbidden"
            && error["details"]["reason"] == "credential_type_unsupported"
            && error["details"]["credential_type"] == "oauth_client"
        {
            return Ok(ConsentRead::UnsupportedCredential);
        }
    }
    if !status.is_success() {
        return Err(unavailable());
    }
    let listing: Consents = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
    let mut matches = listing
        .consents
        .into_iter()
        .filter(|row| row.client_id == client);
    let consent = matches.next();
    if matches.next().is_some() {
        return Err(unavailable());
    }
    if let Some(consent) = &consent {
        consent.validate()?;
    }
    Ok(ConsentRead::Available(consent))
}

#[derive(Deserialize)]
pub(super) struct Discovery {
    issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub introspection_endpoint: String,
}
impl Discovery {
    pub fn validate(&self, issuer: &Url, api_origin: &Url) -> Result<(), StoreError> {
        if Url::parse(&self.issuer).ok().as_ref() != Some(issuer) {
            return Err(unavailable());
        }
        for endpoint in [
            &self.authorization_endpoint,
            &self.token_endpoint,
            &self.introspection_endpoint,
        ] {
            let url = Url::parse(endpoint).map_err(|_| unavailable())?;
            if url.origin() != api_origin.origin()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return Err(unavailable());
            }
        }
        Ok(())
    }
}

#[derive(Deserialize)]
pub(super) struct Consents {
    pub consents: Vec<Consent>,
}
#[derive(Deserialize)]
pub(super) struct Consent {
    pub id: Option<String>,
    pub granted_at: Option<String>,
    pub client_id: String,
    pub scopes: String,
    pub allow_all_services: bool,
    pub allowed_service_ids: BTreeSet<String>,
    pub legacy_unrestricted: bool,
    pub expires_at: Option<String>,
}
impl Consent {
    pub fn retirement_authority_version(
        &self,
    ) -> Result<Option<RetirementAuthorityVersion>, StoreError> {
        let (Some(id), Some(granted_at)) = (&self.id, &self.granted_at) else {
            return Ok(None);
        };
        if self.legacy_unrestricted
            || !self.live()?
            || !self.scopes.split_whitespace().any(|scope| scope == "proxy")
        {
            return Ok(None);
        }
        let grant = uuid::Uuid::parse_str(id)
            .map_err(|_| unavailable())?
            .into_bytes();
        let granted_at_ms = bson::DateTime::parse_rfc3339_str(granted_at)
            .ok()
            .and_then(|stamp| u64::try_from(stamp.timestamp_millis()).ok())
            .filter(|stamp| *stamp > 0)
            .ok_or_else(unavailable)?;
        use sha2::{Digest, Sha256};
        let scopes: BTreeSet<_> = self.scopes.split_whitespace().collect();
        let scope = serde_json::to_vec(&(
            &self.client_id,
            scopes,
            self.allow_all_services,
            &self.allowed_service_ids,
        ))
        .map_err(|_| unavailable())?;
        Ok(Some(RetirementAuthorityVersion {
            grant,
            scope: Sha256::digest(scope).into(),
            granted_at_ms,
            token_generation: None,
        }))
    }

    pub fn validate(&self) -> Result<(), StoreError> {
        if self
            .allowed_service_ids
            .iter()
            .any(|id| uuid::Uuid::parse_str(id).is_err())
            || self.scopes.trim().is_empty()
        {
            return Err(unavailable());
        }
        self.live().map(|_| ())
    }
    pub fn live(&self) -> Result<bool, StoreError> {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_err(|_| unavailable())?
            .as_secs();
        match &self.expires_at {
            None => Ok(true),
            Some(expiry) => {
                Ok(layer2_core::seconds_from_rfc3339(expiry).ok_or_else(unavailable)? > now)
            }
        }
    }
}
#[derive(Deserialize)]
pub(super) struct Services {
    pub services: Vec<Service>,
}
#[derive(Clone, Deserialize)]
pub(super) struct Service {
    pub id: String,
    pub slug: String,
    pub resource_uri: String,
    pub is_active: bool,
    pub credential_source: CredentialSource,
}
#[derive(Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum CredentialSource {
    Personal,
    Org { allowed: bool },
}

pub(super) fn resolve(
    required: &BTreeSet<ServiceId>,
    rows: Vec<Service>,
    issuer: &Url,
) -> Result<BTreeMap<ServiceId, Service>, StoreError> {
    required
        .iter()
        .map(|id| {
            let matches: Vec<_> = rows
                .iter()
                .filter(|row| row.id == id.as_str() || row.slug == id.as_str())
                .collect();
            let [row] = matches.as_slice() else {
                return Err(unavailable());
            };
            let resource = Url::parse(&row.resource_uri).map_err(|_| unavailable())?;
            if !row.is_active
                || matches!(
                    row.credential_source,
                    CredentialSource::Org { allowed: false }
                )
                || uuid::Uuid::parse_str(&row.id).is_err()
                || resource.origin() != issuer.origin()
                || !resource.username().is_empty()
                || resource.password().is_some()
                || resource.query().is_some()
                || resource.fragment().is_some()
            {
                return Err(unavailable());
            }
            Ok((id.clone(), (*row).clone()))
        })
        .collect()
}

pub(super) fn missing(
    consent: Option<&Consent>,
    services: &BTreeMap<ServiceId, Service>,
) -> Result<BTreeSet<ServiceId>, StoreError> {
    let grant = match consent {
        Some(grant) if grant.live()? => Some(grant),
        _ => None,
    };
    if grant.is_some_and(|grant| {
        grant.legacy_unrestricted
            || !grant
                .scopes
                .split_whitespace()
                .any(|scope| scope == "proxy")
    }) {
        return Err(unavailable());
    }
    Ok(services
        .iter()
        .filter(|(_, service)| {
            grant.is_none_or(|grant| {
                !grant.allow_all_services && !grant.allowed_service_ids.contains(&service.id)
            })
        })
        .map(|(id, _)| id.clone())
        .collect())
}

pub(super) fn redirect_allowed(auth: &AuthState, redirect: &str) -> bool {
    Url::parse(redirect).is_ok_and(|url| {
        url.scheme() == "https"
            && url.path() == CALLBACK_PATH
            && url.query().is_none()
            && url.fragment().is_none()
            && url.username().is_empty()
            && url.password().is_none()
            && auth.browser_origins.iter().any(|origin| {
                Url::parse(origin).is_ok_and(|origin| origin.origin() == url.origin())
            })
    })
}

#[derive(Deserialize)]
pub(super) struct Token {
    pub access_token: String,
}
#[derive(Deserialize)]
pub(super) struct Introspection {
    active: bool,
    sub: Option<String>,
    client_id: Option<String>,
    iss: Option<String>,
    token_type: Option<String>,
    exp: Option<u64>,
}
impl Introspection {
    pub fn matches(&self, issuer: &Url, client: &str, subject: &SubjectId) -> bool {
        self.active
            && self.sub.as_deref() == Some(subject.as_str())
            // NyxID omits client_id for ordinary OAuth tokens. The code was
            // exchanged with this client and its exact redirect/PKCE binding.
            && self.client_id.as_deref().is_none_or(|issued| issued == client)
            && self.token_type.as_deref() == Some("access")
            && self
                .iss
                .as_deref()
                .and_then(|s| Url::parse(s).ok())
                .as_ref()
                == Some(issuer)
            && self.exp.is_some_and(|exp| {
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .is_ok_and(|now| exp > now.as_secs())
            })
    }
}
