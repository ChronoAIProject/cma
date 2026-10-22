//! S3 object transport through the existing authenticated storage service.
//! Source paths are content-addressed and writes require its durable generation
//! fence. Neither a storage URL nor a machine credential is returned to users.
use crate::config::DocumentStorageConfig;
use layer2_core::document_artifacts::{
    digest, DocumentBlobStore, DocumentError, MAX_DOCUMENT_BYTES,
};
use reqwest::{Client, Method, Response, StatusCode};
use serde::Deserialize;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use zeroize::Zeroizing;

pub(crate) struct StorageDocuments {
    http: Client,
    config: DocumentStorageConfig,
    token: Mutex<Option<(Zeroizing<String>, Instant)>>,
}
impl StorageDocuments {
    pub(crate) fn new(config: DocumentStorageConfig) -> Result<Arc<Self>, String> {
        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(12))
            .user_agent("cma-document-storage")
            .build()
            .map_err(|_| "document storage HTTP client could not be built")?;
        Ok(Arc::new(Self {
            http,
            config,
            token: Mutex::new(None),
        }))
    }
    async fn token(&self, rejected: Option<&str>) -> Result<Zeroizing<String>, DocumentError> {
        let mut guard = self.token.lock().await;
        if let Some((token, expires)) = guard.as_ref() {
            if *expires > Instant::now() + Duration::from_secs(60)
                && rejected != Some(token.as_str())
            {
                return Ok(token.clone());
            }
        }
        let response = self
            .http
            .post(format!("{}/oauth/token", self.config.platform_origin))
            .form(&[
                ("grant_type", "client_credentials"),
                ("client_id", self.config.client_id.as_str()),
                ("client_secret", self.config.client_secret.as_str()),
                ("scope", "proxy object:create object:read object:download"),
            ])
            .send()
            .await
            .map_err(|_| DocumentError::Unavailable)?;
        if !response.status().is_success() {
            return Err(DocumentError::Unavailable);
        }
        let bytes = bounded(response, 16 * 1024).await?;
        let auth: TokenReply =
            serde_json::from_slice(&bytes).map_err(|_| DocumentError::Unavailable)?;
        if auth.access_token.is_empty()
            || auth.access_token.len() > 16 * 1024
            || auth.expires_in < 120
            || auth.expires_in > 86_400
        {
            return Err(DocumentError::Unavailable);
        }
        let token = Zeroizing::new(auth.access_token);
        *guard = Some((
            token.clone(),
            Instant::now() + Duration::from_secs(auth.expires_in),
        ));
        Ok(token)
    }
    fn object_url(&self, key: &str, write: Option<&str>) -> Result<url::Url, DocumentError> {
        if !valid_key(key) {
            return Err(DocumentError::Invalid);
        }
        let suffix = if write.is_some() { "" } else { "/download" };
        let mut url = url::Url::parse(&format!(
            "{}/api/v1/proxy/s/chrono-storage-service/api/buckets/{}/objects{suffix}",
            self.config.platform_origin, self.config.bucket
        ))
        .map_err(|_| DocumentError::Unavailable)?;
        url.query_pairs_mut().append_pair("key", key);
        if let Some(sha) = write {
            url.query_pairs_mut()
                .append_pair("contentType", "text/markdown; charset=utf-8")
                .append_pair("mutationGeneration", "1")
                .append_pair("contentSha256", sha);
        }
        Ok(url)
    }
    async fn send(
        &self,
        method: Method,
        url: url::Url,
        body: Option<Vec<u8>>,
    ) -> Result<Response, DocumentError> {
        let mut token = self.token(None).await?;
        for attempt in 0..2 {
            let mut request = self
                .http
                .request(method.clone(), url.clone())
                .bearer_auth(token.as_str());
            if let Some(body) = &body {
                request = request
                    .header("content-type", "text/markdown; charset=utf-8")
                    .body(body.clone());
            }
            let response = request
                .send()
                .await
                .map_err(|_| DocumentError::Unavailable)?;
            if response.status() == StatusCode::UNAUTHORIZED && attempt == 0 {
                token = self.token(Some(&token)).await?;
                continue;
            }
            if !response.status().is_success() {
                return Err(DocumentError::Unavailable);
            }
            return Ok(response);
        }
        Err(DocumentError::Unavailable)
    }
}
#[derive(Deserialize)]
struct TokenReply {
    access_token: String,
    expires_in: u64,
}
#[derive(Deserialize)]
struct StoredReply {
    data: StoredMetadata,
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

#[async_trait::async_trait]
impl DocumentBlobStore for StorageDocuments {
    async fn put(&self, key: &str, bytes: Vec<u8>) -> Result<(), DocumentError> {
        if bytes.is_empty() || bytes.len() > MAX_DOCUMENT_BYTES {
            return Err(DocumentError::Invalid);
        }
        let sha = digest(&bytes);
        if !key.ends_with(&format!("/{sha}.md")) {
            return Err(DocumentError::Invalid);
        }
        let size = bytes.len() as u64;
        let response = self
            .send(Method::POST, self.object_url(key, Some(&sha))?, Some(bytes))
            .await?;
        let receipt: StoredReply = serde_json::from_slice(&bounded(response, 16 * 1024).await?)
            .map_err(|_| DocumentError::Corrupt)?;
        if receipt.data.key != key
            || receipt.data.content_length != size
            || receipt.data.generation != 1
            || receipt.data.digest != sha
            || receipt.data.fence_version != 1
        {
            return Err(DocumentError::Corrupt);
        }
        Ok(())
    }
    async fn get(&self, key: &str) -> Result<Vec<u8>, DocumentError> {
        bounded(
            self.send(Method::GET, self.object_url(key, None)?, None)
                .await?,
            MAX_DOCUMENT_BYTES,
        )
        .await
    }
}
async fn bounded(mut response: Response, max: usize) -> Result<Vec<u8>, DocumentError> {
    if response.content_length().is_some_and(|n| n > max as u64) {
        return Err(DocumentError::Corrupt);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| DocumentError::Unavailable)?
    {
        if body.len().saturating_add(chunk.len()) > max {
            return Err(DocumentError::Corrupt);
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
