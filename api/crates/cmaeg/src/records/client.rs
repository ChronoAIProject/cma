//! Service-account client for the CMA gateway-record endpoints.

use std::{fmt, time::Duration};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg_core::{channel::Secret, ids::RecordId};
use reqwest::{Client, StatusCode, Url};
use serde::{Deserialize, Serialize};

#[path = "client_guarded.rs"]
mod guarded;

const RECORD_PATH: &str = "/api/v1/gateway/channel-records";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordValue {
    pub blob: Vec<u8>,
    pub version: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RecordHead {
    pub record_id: String,
    pub owner: String,
    pub version: u64,
}

/// A versioned authority row that must be written with a guarded interaction
/// root. This is crate-private because only routed admission may use it.
#[derive(Clone, Debug)]
pub(crate) struct AuthorityGuard {
    pub owner: String,
    pub kind: &'static str,
    pub id: String,
    pub version: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordError {
    NotFound,
    Conflict { expected: u64, actual: Option<u64> },
    Rejected(String),
    Unavailable(String),
    Malformed(String),
}

impl fmt::Display for RecordError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => formatter.write_str("channel record was not found"),
            Self::Conflict { expected, actual } => {
                write!(
                    formatter,
                    "channel record version conflict: expected {expected}, actual {actual:?}"
                )
            }
            Self::Rejected(detail) => {
                write!(formatter, "channel record request was rejected: {detail}")
            }
            Self::Unavailable(detail) => {
                write!(formatter, "channel record store unavailable: {detail}")
            }
            Self::Malformed(detail) => {
                write!(formatter, "channel record response was malformed: {detail}")
            }
        }
    }
}

impl std::error::Error for RecordError {}

#[derive(Debug, Clone)]
pub struct RecordClient {
    database: Option<super::mongo::MongoRecords>,
    client: Client,
    base_url: Url,
    record_path: &'static str,
    record_kind: super::RecordKind,
    retention: super::retention::InteractionRetention,
    service_token: crate::service_auth::ServiceAuth,
    #[cfg(test)]
    thread_fences_fixture: bool,
    #[cfg(test)]
    whatsapp_reaction_fixture: bool,
    #[cfg(test)]
    media_continuations_fixture: bool,
}

impl RecordClient {
    pub fn new(base_url: &str, service_token: impl Into<String>) -> Result<Self, RecordError> {
        let base_url = Url::parse(base_url)
            .map_err(|error| RecordError::Rejected(format!("invalid CMA URL: {error}")))?;
        if !matches!(base_url.scheme(), "http" | "https")
            || base_url.host_str().is_none()
            || base_url.username() != ""
            || base_url.password().is_some()
            || base_url.query().is_some()
            || base_url.fragment().is_some()
            || (base_url.path() != "" && base_url.path() != "/")
        {
            return Err(RecordError::Rejected(
                "CMA base URL must be an origin".into(),
            ));
        }
        let service_token = service_token.into();
        if service_token.is_empty() {
            return Err(RecordError::Rejected(
                "CMA service token must not be empty".into(),
            ));
        }
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(format!("cmaeg/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| RecordError::Unavailable(error.to_string()))?;
        Ok(Self {
            database: None,
            client,
            base_url,
            record_path: RECORD_PATH,
            record_kind: super::RecordKind::Channel,
            retention: Default::default(),
            service_token: crate::service_auth::ServiceAuth::Static(Secret::new(service_token)),
            #[cfg(test)]
            thread_fences_fixture: false,
            #[cfg(test)]
            whatsapp_reaction_fixture: false,
            #[cfg(test)]
            media_continuations_fixture: false,
        })
    }

    pub fn with_database(mut self, database: super::mongo::MongoRecords) -> Self {
        self.database = Some(database);
        self
    }

    pub(crate) fn worker_credentials(
        &self,
    ) -> Option<crate::worker_credentials::WorkerCredentials> {
        self.database
            .as_ref()
            .and_then(super::mongo::MongoRecords::worker_credentials)
    }

    pub(crate) fn database(&self) -> Option<mongodb::Database> {
        self.database
            .as_ref()
            .map(super::mongo::MongoRecords::database)
    }

    pub(crate) fn with_service_auth(
        mut self,
        service_auth: crate::service_auth::ServiceAuth,
    ) -> Self {
        self.service_token = service_auth;
        self
    }

    pub async fn reachable(&self) -> bool {
        if let Some(db) = &self.database {
            return db.reachable().await;
        }
        let Ok(url) = self.base_url.join("/ready") else {
            return false;
        };
        self.client
            .get(url)
            .timeout(Duration::from_millis(1_500))
            .send()
            .await
            .is_ok_and(|response| response.status().is_success())
    }

    pub fn connections(mut self) -> Self {
        self.record_path = "/api/v1/gateway/connection-records";
        self.record_kind = super::RecordKind::Connection;
        self
    }

    pub fn interactions(mut self) -> Self {
        self.record_path = "/api/v1/gateway/interaction-records";
        self.record_kind = super::RecordKind::Interaction;
        self.retention = Default::default();
        self
    }

    pub fn record_url(&self, record_id: &str) -> Result<Url, RecordError> {
        self.check_thread_fence_transport()?;
        self.check_media_continuation_transport()?;
        let record_path = self.record_path;
        let id = uuid::Uuid::parse_str(record_id)
            .map_err(|_| RecordError::Rejected("invalid record ID".into()))?;
        if id.get_version_num() != 4 || id.to_string() != record_id {
            return Err(RecordError::Rejected(
                "record ID must be a canonical UUID v4".into(),
            ));
        }
        self.base_url
            .join(&format!("{record_path}/{record_id}"))
            .map_err(|error| RecordError::Rejected(format!("record URL is invalid: {error}")))
    }

    pub async fn put(
        &self,
        record_id: &RecordId,
        owner: &str,
        blob: &[u8],
        expected_version: Option<u64>,
    ) -> Result<u64, RecordError> {
        self.put_expiring(record_id, owner, blob, expected_version, None)
            .await
    }

    pub async fn get(&self, record_url: &str) -> Result<RecordValue, RecordError> {
        let record_path = self.record_path;
        let url = Url::parse(record_url)
            .map_err(|error| RecordError::Rejected(format!("record URL is invalid: {error}")))?;
        let id = url
            .path()
            .strip_prefix(&format!("{record_path}/"))
            .ok_or_else(|| RecordError::Rejected("invalid record path".into()))?;
        if self.record_url(id)?.as_str() != record_url
            || url.origin() != self.base_url.origin()
            || !url.path().starts_with(&format!("{record_path}/"))
        {
            return Err(RecordError::Rejected(
                "record URL is outside the configured CMA origin".into(),
            ));
        }
        if let Some(db) = &self.database {
            return db.for_kind(self.record_kind).get(id).await;
        }
        let response = self
            .client
            .get(url)
            .bearer_auth(
                self.service_token
                    .bearer()
                    .await
                    .map_err(|error| RecordError::Unavailable(error.to_string()))?
                    .as_str(),
            )
            .timeout(Duration::from_millis(1_500))
            .send()
            .await
            .map_err(transport)?;
        if response.status() == StatusCode::NOT_FOUND {
            return Err(RecordError::NotFound);
        }
        let response = success(response).await?;
        let body = read_json::<GetResponse>(response).await?;
        let blob = STANDARD.decode(body.blob).map_err(|error| {
            RecordError::Malformed(format!("record blob is not base64: {error}"))
        })?;
        if blob.is_empty() || blob.len() > 65_536 {
            return Err(RecordError::Malformed("invalid blob size".into()));
        }
        Ok(RecordValue {
            blob,
            version: body.version,
        })
    }

    /// Internal bounded watermark lookup; HTTP stores retain the full-read path.
    pub(crate) async fn local_versions(
        &self,
        owner: &str,
        ids: &[String],
    ) -> Result<Option<Vec<u64>>, RecordError> {
        if owner.is_empty() || owner.len() > 512 || ids.is_empty() || ids.len() > 7 {
            return Err(RecordError::Rejected(
                "invalid record watermark lookup".into(),
            ));
        }
        for id in ids {
            self.record_url(id)?;
        }
        let Some(database) = &self.database else {
            return Ok(None);
        };
        tokio::time::timeout(
            Duration::from_secs(2),
            database.for_kind(self.record_kind).versions(owner, ids),
        )
        .await
        .map_err(|_| RecordError::Unavailable("record watermark read timed out".into()))?
        .map(Some)
    }

    pub async fn delete(
        &self,
        record_id: &RecordId,
        expected_version: u64,
    ) -> Result<bool, RecordError> {
        let url = self.record_url(record_id.as_str())?;
        if let Some(db) = &self.database {
            return db
                .for_kind(self.record_kind)
                .delete(record_id.as_str(), expected_version)
                .await;
        }
        let response = self
            .client
            .delete(url)
            .bearer_auth(
                self.service_token
                    .bearer()
                    .await
                    .map_err(|error| RecordError::Unavailable(error.to_string()))?
                    .as_str(),
            )
            .query(&[("expected_version", expected_version)])
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .map_err(transport)?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(false);
        }
        if response.status() == StatusCode::CONFLICT {
            return Err(conflict(response, expected_version).await);
        }
        success(response).await.map(|_| true)
    }

    pub async fn list(
        &self,
        owner: Option<&str>,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<RecordHead>, RecordError> {
        self.check_thread_fence_transport()?;
        self.check_media_continuation_transport()?;
        if let Some(db) = &self.database {
            return db
                .for_kind(self.record_kind)
                .list(owner, after, limit)
                .await;
        }
        if self.record_kind == super::RecordKind::MediaContinuation && !(1..=32).contains(&limit) {
            return Err(RecordError::Rejected(
                "invalid media continuation page size".into(),
            ));
        }
        let url = self
            .base_url
            .join(self.record_path)
            .map_err(|error| RecordError::Rejected(error.to_string()))?;
        let mut query = Vec::<(&str, String)>::new();
        if let Some(owner) = owner {
            query.push(("owner", owner.to_owned()));
        }
        if let Some(after) = after {
            query.push(("after", after.to_owned()));
        }
        query.push(("limit", limit.to_string()));
        let response = self
            .client
            .get(url)
            .bearer_auth(
                self.service_token
                    .bearer()
                    .await
                    .map_err(|error| RecordError::Unavailable(error.to_string()))?
                    .as_str(),
            )
            .query(&query)
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .map_err(transport)?;
        let response = success(response).await?;
        let body = read_json::<ListResponse>(response).await?;
        Ok(body.items)
    }
}

#[derive(Debug, Serialize)]
struct PutRequest<'a> {
    owner: &'a str,
    blob: String,
    expected_version: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct PutResponse {
    version: u64,
}

#[derive(Debug, Deserialize)]
struct GetResponse {
    blob: String,
    version: u64,
}

#[derive(Debug, Deserialize)]
struct ListResponse {
    items: Vec<RecordHead>,
}

async fn success(response: reqwest::Response) -> Result<reqwest::Response, RecordError> {
    if response.status().is_success() {
        Ok(response)
    } else {
        let status = response.status();
        if status.is_client_error() {
            Err(RecordError::Rejected(format!("HTTP {status}")))
        } else {
            Err(RecordError::Unavailable(format!("HTTP {status}")))
        }
    }
}

async fn conflict(response: reqwest::Response, expected: u64) -> RecordError {
    let actual = read_json::<serde_json::Value>(response)
        .await
        .ok()
        .and_then(|body| body.get("actual").and_then(serde_json::Value::as_u64));
    RecordError::Conflict { expected, actual }
}

fn transport(error: reqwest::Error) -> RecordError {
    if error.is_timeout() {
        RecordError::Unavailable("request timed out".into())
    } else {
        RecordError::Unavailable("transport failed".into())
    }
}

async fn read_json<T: serde::de::DeserializeOwned>(
    mut response: reqwest::Response,
) -> Result<T, RecordError> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(transport)? {
        if bytes.len() + chunk.len() > 256 * 1024 {
            return Err(RecordError::Malformed("response exceeds size limit".into()));
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| RecordError::Malformed("invalid response JSON".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn record_urls_are_exact_and_never_send_service_tokens_to_other_origins() {
        let client = RecordClient::new("http://127.0.0.1:1", "service-fixture").unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let valid = client.record_url(&id).unwrap();
        assert!(!format!("{client:?}").contains("service-fixture"));
        for url in [
            format!("https://other.example{RECORD_PATH}/{id}"),
            format!("{valid}?query=1"),
            format!("{valid}#fragment"),
            format!("{valid}/extra"),
            valid.to_string().replace(&id, ".."),
            valid.to_string().replace("127.0.0.1", "localhost"),
        ] {
            assert!(matches!(
                client.get(&url).await,
                Err(RecordError::Rejected(_))
            ));
        }
        assert!(client.record_url("../other").is_err());
        assert!(client.record_url("not-a-uuid").is_err());
    }
}

#[path = "client_thread_fences.rs"]
mod thread_fences;

#[path = "client_whatsapp_flows.rs"]
mod whatsapp_flows;

#[path = "client_write.rs"]
mod write;

#[path = "client_whatsapp_reactions.rs"]
mod whatsapp_reactions;

#[path = "client_media_continuations.rs"]
mod media_continuations;
