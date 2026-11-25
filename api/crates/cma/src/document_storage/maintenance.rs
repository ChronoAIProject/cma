//! Unverified object-maintenance contract, gated independently from online access.
use super::{
    bounded, ensure_budget, valid_key, Client, DocumentError, DocumentStorageConfig, Method,
    Response, StatusCode, StorageToken, CONNECT_TIMEOUT, OPERATION_TIMEOUT, REQUEST_TIMEOUT,
};
use layer2_core::document_artifacts::{
    valid_document_blob_prefix, BlobDeletion, BlobMaintenanceError, BlobPage,
    DocumentBlobMaintenance, MAX_BLOB_CURSOR_BYTES, MAX_BLOB_PAGE_KEYS,
};
use serde::Deserialize;
use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};
use tokio::time::Instant;

mod fenced;
pub(crate) use fenced::ConfirmedDeletion;

// Provisional E1 scope names; storage-owner confirmation and NyxID grants are pending.
const MAINTENANCE_SCOPE: &str = "proxy object:read object:list object:delete";
const MAX_LIST_BYTES: usize = 256 * 1024;
const MAX_RETRY_AFTER: Duration = Duration::from_secs(3600);

pub(crate) struct StorageMaintenance {
    http: Client,
    objects: url::Url,
    enabled: bool,
    pub(super) token: StorageToken,
}

impl StorageMaintenance {
    pub(crate) fn new(config: DocumentStorageConfig, enabled: bool) -> Result<Arc<Self>, String> {
        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .retry(reqwest::retry::never())
            .user_agent("cma-document-maintenance")
            .build()
            .map_err(|_| "document maintenance HTTP client could not be built")?;
        let origin = url::Url::parse(&config.platform_origin)
            .map_err(|_| "document maintenance origin is invalid")?;
        let objects = super::storage_objects_url(&config)?;
        let token = StorageToken::new(
            http.clone(),
            origin,
            config.client_id,
            config.client_secret,
            MAINTENANCE_SCOPE,
        );
        Ok(Arc::new(Self {
            http,
            objects,
            enabled,
            token,
        }))
    }

    pub(crate) fn namespace_sha256(&self) -> String {
        layer2_core::document_artifacts::digest(self.objects.as_str().as_bytes())
    }

    async fn send(
        &self,
        method: Method,
        url: url::Url,
        deadline: Instant,
    ) -> Result<Response, BlobMaintenanceError> {
        let budget =
            || ensure_budget(deadline).map_err(|failure| BlobMaintenanceError::from(failure.error));
        budget()?;
        let (mut token, mut generation) = self.token.current(None).await?;
        for replayed in [false, true] {
            // A token becoming ready at an expired deadline must not start HTTP.
            budget()?;
            let response = self
                .http
                .request(method.clone(), url.clone())
                .bearer_auth(token.as_str())
                .send()
                .await
                .map_err(|_| BlobMaintenanceError::from(DocumentError::Unavailable))?;
            if response.status() == StatusCode::UNAUTHORIZED && !replayed {
                budget()?;
                (token, generation) = self.token.current(Some(generation)).await?;
                continue;
            }
            return Ok(response);
        }
        Err(DocumentError::Unavailable.into())
    }

    async fn delete_object(
        &self,
        key: &str,
        deadline: Instant,
    ) -> Result<BlobDeletion, BlobMaintenanceError> {
        let mut url = self.objects.clone();
        url.query_pairs_mut().append_pair("key", key);
        let response = self.send(Method::DELETE, url, deadline).await?;
        match response.status() {
            StatusCode::OK | StatusCode::NO_CONTENT => Ok(BlobDeletion::Deleted),
            StatusCode::NOT_FOUND => Ok(BlobDeletion::Absent),
            _ => Err(unavailable(&response)),
        }
    }

    async fn list_objects(
        &self,
        prefix: &str,
        cursor: Option<&str>,
        deadline: Instant,
    ) -> Result<BlobPage, BlobMaintenanceError> {
        let mut url = self.objects.clone();
        url.query_pairs_mut()
            .append_pair("prefix", prefix)
            .append_pair("limit", &MAX_BLOB_PAGE_KEYS.to_string());
        if let Some(cursor) = cursor {
            url.query_pairs_mut().append_pair("cursor", cursor);
        }
        let response = self.send(Method::GET, url, deadline).await?;
        if !response.status().is_success() {
            return Err(unavailable(&response));
        }
        let reply: ListReply = serde_json::from_slice(&bounded(response, MAX_LIST_BYTES).await?)
            .map_err(|_| DocumentError::Corrupt)?;
        if reply.error.is_some()
            || reply.data.keys.len() > MAX_BLOB_PAGE_KEYS
            || reply
                .data
                .next
                .as_ref()
                .is_some_and(|next| next.len() > MAX_BLOB_CURSOR_BYTES)
            || reply
                .data
                .keys
                .iter()
                .any(|key| !valid_key(key) || !key.starts_with(prefix))
        {
            return Err(DocumentError::Corrupt.into());
        }
        Ok(BlobPage {
            keys: reply.data.keys,
            next: reply.data.next,
        })
    }
}

#[async_trait::async_trait]
impl DocumentBlobMaintenance for StorageMaintenance {
    async fn delete(&self, key: &str) -> Result<BlobDeletion, BlobMaintenanceError> {
        if !self.enabled {
            return Err(DocumentError::Unavailable.into());
        }
        if !valid_key(key) {
            return Err(DocumentError::Invalid.into());
        }
        let deadline = Instant::now() + OPERATION_TIMEOUT;
        tokio::time::timeout_at(deadline, self.delete_object(key, deadline))
            .await
            .map_err(|_| BlobMaintenanceError::from(DocumentError::Unavailable))?
    }

    async fn list(
        &self,
        prefix: &str,
        cursor: Option<&str>,
    ) -> Result<BlobPage, BlobMaintenanceError> {
        if !self.enabled {
            return Err(DocumentError::Unavailable.into());
        }
        if !valid_document_blob_prefix(prefix)
            || cursor.is_some_and(|cursor| cursor.len() > MAX_BLOB_CURSOR_BYTES)
        {
            return Err(DocumentError::Invalid.into());
        }
        let deadline = Instant::now() + OPERATION_TIMEOUT;
        tokio::time::timeout_at(deadline, self.list_objects(prefix, cursor, deadline))
            .await
            .map_err(|_| BlobMaintenanceError::from(DocumentError::Unavailable))?
    }

    fn deletion_supported(&self) -> bool {
        self.enabled
    }
}

#[derive(Deserialize)]
struct ListReply {
    data: ListData,
    error: Option<serde_json::Value>,
}
#[derive(Deserialize)]
struct ListData {
    keys: Vec<String>,
    next: Option<String>,
}

fn unavailable(response: &Response) -> BlobMaintenanceError {
    let retry_after = if response.status() == StatusCode::TOO_MANY_REQUESTS {
        response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| {
                if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
                    value.parse::<u64>().ok().map(Duration::from_secs)
                } else {
                    httpdate::parse_http_date(value)
                        .ok()
                        .map(|at| at.duration_since(SystemTime::now()).unwrap_or_default())
                }
            })
            .map(|delay| delay.min(MAX_RETRY_AFTER))
    } else {
        None
    };
    BlobMaintenanceError {
        error: DocumentError::Unavailable,
        retry_after,
    }
}
