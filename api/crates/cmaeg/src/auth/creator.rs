//! NyxID-backed creator authentication with a bounded short-lived cache.

use std::{
    fmt,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use axum::{
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use cmaeg_core::problem::{Problem, ProblemCode};
use dashmap::DashMap;
use reqwest::Client;
use ring::digest::{digest, SHA256};
use serde::Deserialize;
// Tokio's clock is the process clock outside tests; under a paused test
// runtime it lets a caller's test move the cache past its TTLs without
// waiting a real minute.
use tokio::time::Instant;

const SUCCESS_TTL: Duration = Duration::from_secs(60);
const FAILURE_TTL: Duration = Duration::from_secs(10);
const CACHE_LIMIT: usize = 10_000;

/// The authenticated NyxID subject and a non-secret token digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Creator {
    pub subject: String,
    pub token_hash: String,
    pub organization: bool,
}

/// State types expose the creator authenticator to the generic extractor.
pub trait CreatorAuthState {
    fn creator_auth(&self) -> &CreatorAuthenticator;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreatorAuthError {
    NotAuthenticated,
    ProviderUnavailable,
    Forbidden,
}

impl CreatorAuthError {
    fn problem(&self) -> Problem {
        match self {
            Self::NotAuthenticated => Problem::new(
                ProblemCode::NotAuthenticated,
                StatusCode::UNAUTHORIZED.as_u16(),
                "A valid NyxID bearer token is required.",
            ),
            Self::Forbidden => Problem::new(
                ProblemCode::NotAuthenticated,
                403,
                "Event Gateway permission is required.",
            ),
            Self::ProviderUnavailable => Problem::new(
                ProblemCode::ProviderUnavailable,
                StatusCode::SERVICE_UNAVAILABLE.as_u16(),
                "NyxID authentication is temporarily unavailable.",
            ),
        }
    }
}

impl fmt::Display for CreatorAuthError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::NotAuthenticated => "creator is not authenticated",
            Self::ProviderUnavailable => "creator authentication provider is unavailable",
            Self::Forbidden => "Event Gateway permission is required",
        })
    }
}

impl std::error::Error for CreatorAuthError {}

impl IntoResponse for CreatorAuthError {
    fn into_response(self) -> Response {
        crate::telemetry::http_failure::FailureStage::CreatorAuthentication
            .response(crate::http::problems::problem_document(self.problem()))
    }
}

#[path = "creator_cancellation.rs"]
mod cancellation;
pub(crate) use cancellation::CancelCreateOwner;
#[path = "creator_integration.rs"]
mod integration;
#[path = "creator_request.rs"]
mod request;
pub use integration::{Administrator, IntegrationOwner};

#[derive(Debug, Clone)]
pub struct CreatorAuthenticator {
    service: Option<cma_service_auth::NyxIdAuth>,
    owner_keys: Option<super::owner_keys::OwnerKeyAuthenticator>,
    client: Client,
    base_url: String,
    cache: std::sync::Arc<DashMap<String, CacheEntry>>,
    sequence: std::sync::Arc<AtomicU64>,
}

#[derive(Debug, Clone)]
struct CacheEntry {
    result: Result<(String, bool), CreatorAuthError>,
    expires_at: Instant,
    sequence: u64,
}

impl CreatorAuthenticator {
    pub fn new(base_url: &str) -> Result<Self, reqwest::Error> {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(format!("cmaeg/{}", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self {
            service: None,
            owner_keys: None,
            client,
            base_url: base_url.trim_end_matches('/').to_owned(),
            cache: std::sync::Arc::new(DashMap::new()),
            sequence: std::sync::Arc::new(AtomicU64::new(0)),
        })
    }

    pub fn with_service(mut self, service: cma_service_auth::NyxIdAuth) -> Self {
        self.service = Some(service);
        self
    }
    pub(crate) fn with_owner_keys(
        mut self,
        auth: super::owner_keys::OwnerKeyAuthenticator,
    ) -> Self {
        self.owner_keys = Some(auth);
        self
    }
    async fn authenticate_owner_key(
        &self,
        headers: &HeaderMap,
        permission: &str,
    ) -> Result<Creator, CreatorAuthError> {
        // Ordinary keys identify a personal subject. Header-selected organization
        // authority must never widen that scope, including through SSE refresh.
        if headers.contains_key("x-cmaeg-owner") {
            return Err(CreatorAuthError::Forbidden);
        }
        let auth = self
            .owner_keys
            .as_ref()
            .ok_or(CreatorAuthError::NotAuthenticated)?;
        let hash = token_hash(bearer(headers).ok_or(CreatorAuthError::NotAuthenticated)?);
        let principal = auth.authenticate(headers, &hash, permission).await?;
        Ok(Creator {
            subject: principal.subject,
            token_hash: hash,
            organization: false,
        })
    }
    /// Identify a NyxID bearer and, when `owner` is given, verify the bearer may act
    /// for that organization, through the shared 60 s cache. REST calls it after parsing
    /// headers; the console calls it with a session's token on every request.
    pub async fn authenticate_token(
        &self,
        token: &str,
        owner: Option<&str>,
    ) -> Result<Creator, CreatorAuthError> {
        if cma_service_auth::is_service_key(token) {
            if owner.is_some() {
                return Err(CreatorAuthError::Forbidden);
            }
            let mut headers = HeaderMap::new();
            headers.insert(
                axum::http::header::AUTHORIZATION,
                format!("Bearer {token}")
                    .parse()
                    .map_err(|_| CreatorAuthError::NotAuthenticated)?,
            );
            return self
                .authenticate_owner_key(&headers, "cma-eg:channels:read")
                .await;
        }
        let token_hash = token_hash(token);
        let cache_key = owner.map_or_else(
            || token_hash.clone(),
            |owner| format!("{token_hash}:{owner}"),
        );
        if let Some(entry) = self.cache.get(&cache_key) {
            if entry.expires_at > Instant::now() {
                return entry.result.clone().map(|(subject, organization)| Creator {
                    subject,
                    organization,
                    token_hash: token_hash.clone(),
                });
            }
            drop(entry);
            self.cache.remove(&cache_key);
        }
        let started = Instant::now();
        let result = self.fetch_verified(token, owner).await;
        let (ttl, cached) = match &result {
            Ok(creator) => (
                SUCCESS_TTL,
                Ok((creator.subject.clone(), creator.organization)),
            ),
            Err(error) => (FAILURE_TTL, Err(error.clone())),
        };
        self.insert_cache(
            cache_key,
            CacheEntry {
                result: cached,
                expires_at: started + ttl,
                sequence: self.sequence.fetch_add(1, Ordering::Relaxed),
            },
        );
        result
    }

    // The one identity and organization-admin rule, under its 3 s timeout; only
    // `authenticate_token` calls it, so every caller shares the cache.
    async fn fetch_verified(
        &self,
        token: &str,
        owner: Option<&str>,
    ) -> Result<Creator, CreatorAuthError> {
        let result = tokio::time::timeout(Duration::from_secs(3), self.fetch_creator(token, owner))
            .await
            .unwrap_or(Err(CreatorAuthError::ProviderUnavailable));
        result.map(|(subject, organization)| Creator {
            subject,
            organization,
            token_hash: token_hash(token),
        })
    }

    async fn fetch_creator(
        &self,
        token: &str,
        owner: Option<&str>,
    ) -> Result<(String, bool), CreatorAuthError> {
        let (subject, organization) = self.fetch_subject(token).await?;
        let Some(owner) = owner else {
            return Ok((subject, organization));
        };
        // An organization credential cannot select another organization. A human
        // selects only an organization for which this exact bearer is a live admin.
        if organization {
            return if owner == subject {
                Ok((subject, true))
            } else {
                Err(CreatorAuthError::NotAuthenticated)
            };
        }
        if !self.verify_organization(token, owner).await? {
            return Err(CreatorAuthError::NotAuthenticated);
        }
        Ok((owner.to_owned(), true))
    }

    async fn fetch_subject(&self, token: &str) -> Result<(String, bool), CreatorAuthError> {
        let response = self
            .client
            .get(format!("{}/api/v1/users/me", self.base_url))
            .bearer_auth(token)
            .timeout(Duration::from_secs(3))
            .send()
            .await
            .map_err(|_| CreatorAuthError::ProviderUnavailable)?;
        // Organization keys intentionally cannot read a personal /users/me
        // profile. Authenticate their subject using NyxID's userinfo endpoint,
        // then require independent positive organization evidence.
        if response.status() == StatusCode::FORBIDDEN {
            let response = self
                .client
                .get(format!("{}/oauth/userinfo", self.base_url))
                .bearer_auth(token)
                .send()
                .await
                .map_err(|_| CreatorAuthError::ProviderUnavailable)?;
            let body = read_subject(response).await?;
            if !self.verify_organization(token, &body.subject).await? {
                return Err(CreatorAuthError::NotAuthenticated);
            }
            return Ok((body.subject, true));
        }
        let body = read_subject(response).await?;
        let organization = match body.user_type.as_deref() {
            Some("org") => true,
            Some("person") => false,
            None => self.verify_organization(token, &body.subject).await?,
            Some(_) => return Err(CreatorAuthError::ProviderUnavailable),
        };
        Ok((body.subject, organization))
    }

    // Older NyxID profiles omit user_type. A positively verified organization
    // endpoint supplies the same evidence; never infer type from an ID string.
    async fn verify_organization(
        &self,
        token: &str,
        subject: &str,
    ) -> Result<bool, CreatorAuthError> {
        let mut url =
            url::Url::parse(&self.base_url).map_err(|_| CreatorAuthError::ProviderUnavailable)?;
        url.path_segments_mut()
            .map_err(|_| CreatorAuthError::ProviderUnavailable)?
            .extend(["api", "v1", "orgs", subject, "authorization"]);
        let response = self
            .client
            .get(url)
            .bearer_auth(token)
            .timeout(Duration::from_secs(3))
            .send()
            .await
            .map_err(|_| CreatorAuthError::ProviderUnavailable)?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(false);
        }
        #[derive(Deserialize)]
        struct Proof {
            id: String,
            your_role: String,
        }
        let proof: Proof = read_json(response).await?;
        if proof.id != subject || proof.your_role != "admin" {
            return Err(CreatorAuthError::NotAuthenticated);
        }
        Ok(true)
    }

    fn insert_cache(&self, key: String, value: CacheEntry) {
        self.cache.insert(key, value);
        while self.cache.len() > CACHE_LIMIT {
            let oldest = self
                .cache
                .iter()
                .min_by_key(|entry| entry.sequence)
                .map(|entry| entry.key().clone());
            if let Some(oldest) = oldest {
                self.cache.remove(&oldest);
            } else {
                break;
            }
        }
    }
}

#[derive(Debug, Deserialize)]
struct SubjectResponse {
    #[serde(default)]
    user_type: Option<String>,
    #[serde(alias = "id", alias = "sub")]
    subject: String,
}

async fn read_subject(response: reqwest::Response) -> Result<SubjectResponse, CreatorAuthError> {
    let body: SubjectResponse = read_json(response).await?;
    if body.subject.is_empty() || body.subject.len() > 512 {
        return Err(CreatorAuthError::NotAuthenticated);
    }
    Ok(body)
}

async fn read_json<T: serde::de::DeserializeOwned>(
    mut response: reqwest::Response,
) -> Result<T, CreatorAuthError> {
    if matches!(
        response.status(),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ) {
        return Err(CreatorAuthError::NotAuthenticated);
    }
    if !response.status().is_success() {
        return Err(CreatorAuthError::ProviderUnavailable);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| CreatorAuthError::ProviderUnavailable)?
    {
        if bytes.len() + chunk.len() > 64 * 1024 {
            return Err(CreatorAuthError::ProviderUnavailable);
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| CreatorAuthError::ProviderUnavailable)
}

fn selected_owner(headers: &HeaderMap) -> Result<Option<&str>, CreatorAuthError> {
    let mut values = headers.get_all("x-cmaeg-owner").iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    let value = value
        .to_str()
        .map_err(|_| CreatorAuthError::NotAuthenticated)?;
    if values.next().is_some()
        || value.is_empty()
        || value.len() > 512
        || value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(CreatorAuthError::NotAuthenticated);
    }
    Ok(Some(value))
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    if headers
        .get_all(axum::http::header::AUTHORIZATION)
        .iter()
        .count()
        != 1
    {
        return None;
    }
    let value = headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    let token = value.strip_prefix("Bearer ")?;
    (!token.is_empty() && token.len() <= 16_384 && !token.contains(char::is_whitespace))
        .then_some(token)
}

fn token_hash(token: &str) -> String {
    let bytes = digest(&SHA256, token.as_bytes());
    format!(
        "sha256:{}",
        bytes
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    )
}

pub(super) fn service_error(error: cma_service_auth::AuthError) -> CreatorAuthError {
    match error {
        cma_service_auth::AuthError::Unauthenticated => CreatorAuthError::NotAuthenticated,
        cma_service_auth::AuthError::Forbidden => CreatorAuthError::Forbidden,
        _ => CreatorAuthError::ProviderUnavailable,
    }
}

#[cfg(test)]
#[path = "creator_tests.rs"]
mod tests;
