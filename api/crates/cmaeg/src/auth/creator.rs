//! NyxID-backed creator authentication with a bounded short-lived cache.

use std::{
    fmt,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

use axum::{
    extract::FromRequestParts,
    http::{request::Parts, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use cmaeg_core::problem::{Problem, ProblemCode};
use dashmap::DashMap;
use reqwest::Client;
use ring::digest::{digest, SHA256};
use serde::Deserialize;

const SUCCESS_TTL: Duration = Duration::from_secs(60);
const FAILURE_TTL: Duration = Duration::from_secs(10);
const CACHE_LIMIT: usize = 10_000;

/// The authenticated NyxID subject and a non-secret token digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Creator {
    pub subject: String,
    pub token_hash: String,
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
        let problem = self.problem();
        let mut response = (
            StatusCode::from_u16(problem.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(problem),
        )
            .into_response();
        response.headers_mut().insert(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("application/problem+json"),
        );
        response
    }
}

impl<S> FromRequestParts<S> for Creator
where
    S: CreatorAuthState + Send + Sync,
{
    type Rejection = CreatorAuthError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let auth = state.creator_auth();
        if let Some(service) = &auth.service {
            let principal = service
                .authenticate(&parts.headers)
                .await
                .map_err(service_error)?;
            let permission = match parts.method {
                axum::http::Method::GET | axum::http::Method::HEAD => "cma-eg:channels:read",
                axum::http::Method::DELETE => "cma-eg:channels:delete",
                axum::http::Method::POST if parts.uri.path().ends_with("/test") => {
                    "cma-eg:channels:test"
                }
                axum::http::Method::POST => "cma-eg:channels:create",
                _ => "cma-eg:channels:update",
            };
            principal.require(permission).map_err(service_error)?;
            return Ok(Creator {
                subject: principal.subject,
                token_hash: token_hash(
                    bearer(&parts.headers).ok_or(CreatorAuthError::NotAuthenticated)?,
                ),
            });
        }
        auth.authenticate(&parts.headers).await
    }
}

#[derive(Debug, Clone)]
pub struct CreatorAuthenticator {
    service: Option<cma_service_auth::NyxIdAuth>,
    client: Client,
    base_url: String,
    cache: std::sync::Arc<DashMap<String, CacheEntry>>,
    sequence: std::sync::Arc<AtomicU64>,
}

#[derive(Debug, Clone)]
struct CacheEntry {
    result: Result<String, CreatorAuthError>,
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
    pub async fn authenticate(&self, headers: &HeaderMap) -> Result<Creator, CreatorAuthError> {
        if let Some(service) = &self.service {
            let principal = service.authenticate(headers).await.map_err(service_error)?;
            principal
                .require("cma-eg:channels:read")
                .map_err(service_error)?;
            return Ok(Creator {
                subject: principal.subject,
                token_hash: token_hash(bearer(headers).ok_or(CreatorAuthError::NotAuthenticated)?),
            });
        }
        let token = bearer(headers).ok_or(CreatorAuthError::NotAuthenticated)?;
        let token_hash = token_hash(token);
        if let Some(entry) = self.cache.get(&token_hash) {
            if entry.expires_at > Instant::now() {
                return entry.result.clone().map(|subject| Creator {
                    subject,
                    token_hash: token_hash.clone(),
                });
            }
            drop(entry);
            self.cache.remove(&token_hash);
        }
        let result = self.fetch_subject(token).await;
        let (ttl, cached) = match &result {
            Ok(subject) => (SUCCESS_TTL, Ok(subject.clone())),
            Err(error) => (FAILURE_TTL, Err(error.clone())),
        };
        self.insert_cache(
            token_hash.clone(),
            CacheEntry {
                result: cached,
                expires_at: Instant::now() + ttl,
                sequence: self.sequence.fetch_add(1, Ordering::Relaxed),
            },
        );
        result.map(|subject| Creator {
            subject,
            token_hash,
        })
    }

    async fn fetch_subject(&self, token: &str) -> Result<String, CreatorAuthError> {
        let mut response = self
            .client
            .get(format!("{}/api/v1/users/me", self.base_url))
            .bearer_auth(token)
            .timeout(Duration::from_secs(3))
            .send()
            .await
            .map_err(|_| CreatorAuthError::ProviderUnavailable)?;
        if response.status() == StatusCode::UNAUTHORIZED
            || response.status() == StatusCode::FORBIDDEN
        {
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
        let body: SubjectResponse =
            serde_json::from_slice(&bytes).map_err(|_| CreatorAuthError::ProviderUnavailable)?;
        if body.subject.is_empty() || body.subject.len() > 512 {
            return Err(CreatorAuthError::NotAuthenticated);
        }
        Ok(body.subject)
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
    #[serde(alias = "id")]
    subject: String,
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
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

fn service_error(error: cma_service_auth::AuthError) -> CreatorAuthError {
    match error {
        cma_service_auth::AuthError::Unauthenticated => CreatorAuthError::NotAuthenticated,
        cma_service_auth::AuthError::Forbidden => CreatorAuthError::Forbidden,
        _ => CreatorAuthError::ProviderUnavailable,
    }
}

#[cfg(test)]
#[path = "creator_tests.rs"]
mod tests;
