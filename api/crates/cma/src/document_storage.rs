//! S3 object transport through the existing authenticated storage service.
//! Source paths are content-addressed and writes require its durable generation
//! fence. Neither a storage URL nor a machine credential is returned to users.
use crate::config::DocumentStorageConfig;
use layer2_core::document_artifacts::{
    digest, DocumentBlobStore, DocumentError, DocumentSourceWrite, MAX_DOCUMENT_BYTES,
};
use reqwest::{Client, Method, Response, StatusCode};
use serde::Deserialize;
use std::{sync::Arc, time::Duration};
use tokio::time::Instant;

mod legacy;
pub(crate) mod maintenance;
pub(crate) mod token;
use token::StorageToken;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(12);
// Includes token-state/refresh waits, retries, jitter and response body reads.
const OPERATION_TIMEOUT: Duration = Duration::from_secs(30);
const GET_TRANSPORT_ATTEMPTS: usize = 2;
const MIN_RETRY_BUDGET: Duration = Duration::from_secs(3);

fn storage_objects_url(config: &DocumentStorageConfig) -> Result<url::Url, String> {
    let mut objects = url::Url::parse(&config.platform_origin)
        .map_err(|_| "document storage origin is invalid")?;
    objects.set_path(&format!(
        "/api/v1/proxy/s/chrono-storage-service/api/buckets/{}/objects",
        config.bucket
    ));
    Ok(objects)
}

pub(crate) fn namespace_sha256(config: &DocumentStorageConfig) -> Result<String, String> {
    Ok(digest(storage_objects_url(config)?.as_str().as_bytes()))
}

pub(crate) struct StorageDocuments {
    http: Client,
    config: DocumentStorageConfig,
    read_only: bool,
    token: StorageToken,
    legacy_token: legacy::SerializedToken,
}
impl StorageDocuments {
    pub(crate) fn new(config: DocumentStorageConfig) -> Result<Arc<Self>, String> {
        Self::with_access(config, false)
    }
    pub(crate) fn read_only(config: DocumentStorageConfig) -> Result<Arc<Self>, String> {
        Self::with_access(config, true)
    }
    fn with_access(config: DocumentStorageConfig, read_only: bool) -> Result<Arc<Self>, String> {
        let builder = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .user_agent("cma-document-storage");
        let builder = if config.resilience_enabled {
            builder.retry(reqwest::retry::never())
        } else {
            builder
        };
        let http = builder
            .build()
            .map_err(|_| "document storage HTTP client could not be built")?;
        let origin = url::Url::parse(&config.platform_origin)
            .map_err(|_| "document storage origin is invalid")?;
        let token = StorageToken::new(
            http.clone(),
            origin,
            config.client_id.clone(),
            config.client_secret.clone(),
            if read_only {
                "proxy object:read object:download"
            } else {
                "proxy object:create object:read object:download"
            },
        );
        Ok(Arc::new(Self {
            http,
            config,
            read_only,
            token,
            legacy_token: Default::default(),
        }))
    }
    fn object_url(
        &self,
        key: &str,
        write: Option<(&str, u64, bool)>,
    ) -> Result<url::Url, DocumentError> {
        if !valid_key(key) {
            return Err(DocumentError::Invalid);
        }
        let mut url = storage_objects_url(&self.config).map_err(|_| DocumentError::Unavailable)?;
        if write.is_none() {
            url.path_segments_mut()
                .map_err(|_| DocumentError::Unavailable)?
                .push("download");
        }
        url.query_pairs_mut().append_pair("key", key);
        if let Some((sha, generation, require_frame)) = write {
            if require_frame {
                url.path_segments_mut()
                    .map_err(|_| DocumentError::Unavailable)?
                    .push("fenced-v2");
            }
            url.query_pairs_mut()
                .append_pair("contentType", "text/markdown; charset=utf-8")
                .append_pair("mutationGeneration", &generation.to_string())
                .append_pair("contentSha256", sha);
            if generation > 1 {
                url.query_pairs_mut().append_pair("allowRecreate", "true");
            }
        }
        Ok(url)
    }
    async fn send(
        &self,
        method: Method,
        url: url::Url,
        body: Option<Vec<u8>>,
        replayed: &mut bool,
        deadline: Instant,
    ) -> Result<Response, RequestFailure> {
        if !self.config.resilience_enabled {
            return self.legacy_send(method, url, body).await;
        }
        ensure_budget(deadline)?;
        let (mut token, mut generation) = self.token.current(None).await?;
        loop {
            // Timeout polls its inner future first. Do not enqueue a request
            // when a token or retry delay becomes ready at an expired deadline.
            ensure_budget(deadline)?;
            let mut request = self
                .http
                .request(method.clone(), url.clone())
                .bearer_auth(token.as_str());
            if let Some(body) = &body {
                request = request
                    .header("content-type", "text/markdown; charset=utf-8")
                    .body(body.clone());
            }
            let response = request.send().await?;
            if response.status() == StatusCode::UNAUTHORIZED && !*replayed {
                ensure_budget(deadline)?;
                *replayed = true;
                (token, generation) = self.token.current(Some(generation)).await?;
                continue;
            }
            if !response.status().is_success() {
                return Err(RequestFailure {
                    error: DocumentError::Unavailable,
                    retryable: matches!(response.status().as_u16(), 502..=504),
                });
            }
            return Ok(response);
        }
    }
}
#[derive(Deserialize)]
struct StoredReply {
    data: StoredMetadata,
    error: Option<serde_json::Value>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredMetadata {
    key: String,
    content_length: u64,
    generation: u64,
    digest: String,
    fence_version: u64,
}

impl StorageDocuments {
    async fn put_object(
        &self,
        key: &str,
        bytes: Vec<u8>,
        generation: u64,
        require_frame: bool,
        deadline: Instant,
    ) -> Result<(), DocumentError> {
        if bytes.is_empty()
            || bytes.len() > MAX_DOCUMENT_BYTES
            || generation == 0
            || generation > layer2_core::document_artifacts::MAX_SOURCE_GENERATION
        {
            return Err(DocumentError::Invalid);
        }
        let sha = digest(&bytes);
        if !key.ends_with(&format!("/{sha}.md")) {
            return Err(DocumentError::Invalid);
        }
        let size = bytes.len() as u64;
        let mut replayed = false;
        let response = self
            .send(
                Method::POST,
                self.object_url(key, Some((&sha, generation, require_frame)))?,
                Some(bytes),
                &mut replayed,
                deadline,
            )
            .await
            .map_err(|failure| failure.error)?;
        let receipt: StoredReply = serde_json::from_slice(&bounded(response, 16 * 1024).await?)
            .map_err(|_| DocumentError::Corrupt)?;
        if receipt.error.is_some() {
            return Err(DocumentError::Corrupt);
        }
        let mut mismatch = false;
        for (field, matches) in [
            ("key", receipt.data.key == key),
            ("length", receipt.data.content_length == size),
            ("generation", receipt.data.generation == generation),
            ("digest", receipt.data.digest == sha),
            (
                "fence",
                if require_frame {
                    receipt.data.fence_version == 2
                } else {
                    matches!(receipt.data.fence_version, 1 | 2)
                },
            ),
        ] {
            if !matches {
                mismatch = true;
                if self.config.resilience_enabled {
                    metrics::counter!("cma_document_storage_receipt_mismatch_total", "field" => field)
                        .increment(1);
                    tracing::warn!(parent: None, field, "document storage receipt mismatch");
                }
            }
        }
        if mismatch {
            return Err(DocumentError::Corrupt);
        }
        Ok(())
    }
    async fn get_object(&self, key: &str, deadline: Instant) -> Result<Vec<u8>, DocumentError> {
        let url = self.object_url(key, None)?;
        let mut replayed = false;
        for attempt in 0..GET_TRANSPORT_ATTEMPTS {
            let result = async {
                read_body(
                    self.send(Method::GET, url.clone(), None, &mut replayed, deadline)
                        .await?,
                    MAX_DOCUMENT_BYTES,
                )
                .await
            }
            .await;
            match result {
                Ok(body) => return Ok(body),
                Err(failure)
                    if self.config.resilience_enabled
                        && failure.retryable
                        && attempt + 1 < GET_TRANSPORT_ATTEMPTS
                        && deadline.saturating_duration_since(Instant::now())
                            >= MIN_RETRY_BUDGET =>
                {
                    let random = uuid::Uuid::new_v4().as_u128() as u64;
                    tokio::time::sleep(Duration::from_millis(100 + random % 201)).await;
                }
                Err(failure) => return Err(failure.error),
            }
        }
        Err(DocumentError::Unavailable)
    }

    async fn put_at_generation(
        &self,
        key: &str,
        bytes: Vec<u8>,
        generation: u64,
        require_frame: bool,
    ) -> Result<(), DocumentError> {
        if self.read_only {
            return Err(DocumentError::Invalid);
        }
        let deadline = Instant::now() + OPERATION_TIMEOUT;
        tokio::time::timeout_at(
            deadline,
            self.put_object(key, bytes, generation, require_frame, deadline),
        )
        .await
        .map_err(|_| DocumentError::Unavailable)?
    }
}

#[async_trait::async_trait]
impl DocumentBlobStore for StorageDocuments {
    async fn put(&self, key: &str, bytes: Vec<u8>) -> Result<(), DocumentError> {
        self.put_at_generation(key, bytes, 1, false).await
    }

    async fn put_source(
        &self,
        write: &DocumentSourceWrite,
        bytes: Vec<u8>,
    ) -> Result<(), DocumentError> {
        write.source.validate()?;
        if write.source.bytes != bytes.len() as u64 {
            return Err(DocumentError::Invalid);
        }
        self.put_at_generation(
            &write.source.object_key(),
            bytes,
            write.generation(),
            write.fence.is_some(),
        )
        .await
    }

    async fn get(&self, key: &str) -> Result<Vec<u8>, DocumentError> {
        let deadline = Instant::now() + OPERATION_TIMEOUT;
        tokio::time::timeout_at(deadline, self.get_object(key, deadline))
            .await
            .map_err(|_| DocumentError::Unavailable)?
    }
}
struct RequestFailure {
    error: DocumentError,
    retryable: bool,
}
fn ensure_budget(deadline: Instant) -> Result<(), RequestFailure> {
    if Instant::now() >= deadline {
        return Err(DocumentError::Unavailable.into());
    }
    Ok(())
}
impl From<DocumentError> for RequestFailure {
    fn from(error: DocumentError) -> Self {
        Self {
            error,
            retryable: false,
        }
    }
}
impl From<reqwest::Error> for RequestFailure {
    fn from(error: reqwest::Error) -> Self {
        Self {
            error: DocumentError::Unavailable,
            retryable: error.is_connect()
                || error.is_timeout()
                || error.is_request()
                || error.is_body(),
        }
    }
}
async fn bounded(response: Response, max: usize) -> Result<Vec<u8>, DocumentError> {
    read_body(response, max)
        .await
        .map_err(|failure| failure.error)
}
async fn read_body(mut response: Response, max: usize) -> Result<Vec<u8>, RequestFailure> {
    if response.content_length().is_some_and(|n| n > max as u64) {
        return Err(DocumentError::Corrupt.into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len().saturating_add(chunk.len()) > max {
            return Err(DocumentError::Corrupt.into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}
fn valid_key(key: &str) -> bool {
    let parts: Vec<_> = key.split('/').collect();
    let hex = |value: &str, size: usize| {
        value.len() == size
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    };
    parts.len() == 4
        && parts[0] == "documents"
        && hex(parts[1], 64)
        && parts[2].strip_prefix("doc_").is_some_and(|s| hex(s, 32))
        && parts[3].strip_suffix(".md").is_some_and(|s| hex(s, 64))
}

/// Existing deployments can roll out code before supplying the storage config.
/// No memory-only substitute ever claims that a document has been saved.
pub(crate) struct UnavailableDocuments;
#[async_trait::async_trait]
impl DocumentBlobStore for UnavailableDocuments {
    async fn put(&self, _: &str, _: Vec<u8>) -> Result<(), DocumentError> {
        Err(DocumentError::Unavailable)
    }
    async fn get(&self, _: &str) -> Result<Vec<u8>, DocumentError> {
        Err(DocumentError::Unavailable)
    }
}

#[cfg(test)]
#[path = "document_storage_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "document_storage_failure_tests.rs"]
mod failure_tests;

#[cfg(test)]
#[path = "document_storage_token_tests.rs"]
mod token_tests;

#[cfg(test)]
#[path = "document_storage_transport_tests.rs"]
mod transport_tests;

#[cfg(test)]
#[path = "document_storage_receipt_tests.rs"]
mod receipt_tests;

#[cfg(test)]
#[path = "document_storage_maintenance_tests.rs"]
mod maintenance_tests;

#[cfg(test)]
#[path = "document_storage_maintenance_retry_tests.rs"]
mod maintenance_retry_tests;

#[cfg(test)]
#[path = "document_storage_disabled_tests.rs"]
mod disabled_tests;
