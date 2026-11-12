use super::*;
use reqwest::{header::HeaderValue, Method, Response, StatusCode};
use serde_json::Value;
use std::{path::PathBuf, time::Duration};

const PERMISSIONS: &[&str] = &[
    "storage.buckets.get",
    "storage.objects.create",
    "storage.objects.get",
    "storage.objects.list",
    "storage.objects.getIamPolicy",
    "storage.objects.restore",
    "storage.objects.move",
    "storage.buckets.update",
    "storage.buckets.delete",
    "storage.buckets.setIamPolicy",
    "storage.objects.delete",
    "storage.objects.update",
    "storage.objects.setIamPolicy",
    "storage.objects.overrideUnlockedRetention",
    "storage.objects.setRetention",
];

pub struct GcsCustody {
    http: reqwest::Client,
    bucket: String,
    token_file: Option<PathBuf>,
    verifier_file: PathBuf,
    operation: String,
    origin: url::Url,
}
impl GcsCustody {
    #[cfg(test)]
    pub(super) fn test_origin(&mut self, origin: url::Url) {
        self.origin = origin;
    }
    pub fn new(
        bucket: String,
        operation: String,
        token_file: Option<PathBuf>,
        verifier_file: PathBuf,
    ) -> Result<Self> {
        key(&operation, b"validate")?;
        if !(3..=63).contains(&bucket.len())
            || !bucket
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
            || bucket.starts_with('-')
            || bucket.ends_with('-')
            || token_file.as_ref().is_some_and(|p| !p.is_absolute())
            || !verifier_file.is_absolute()
            || token_file.as_ref() == Some(&verifier_file)
        {
            return Err(REFUSED);
        }
        Ok(Self {
            http: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(120))
                .build()
                .map_err(|_| REFUSED)?,
            bucket,
            operation,
            token_file,
            verifier_file,
            origin: url::Url::parse("https://storage.googleapis.com/").map_err(|_| REFUSED)?,
        })
    }
    fn url(&self, upload: bool, key: Option<&str>) -> Result<url::Url> {
        let mut url = self.origin.clone();
        let mut parts = url.path_segments_mut().map_err(|_| REFUSED)?;
        parts.clear();
        if upload {
            parts.push("upload");
        }
        parts.extend(["storage", "v1", "b", &self.bucket]);
        if let Some(key) = key {
            if !valid_key(key) || key.split('/').nth(1) != Some(self.operation.as_str()) {
                return Err(REFUSED);
            }
            parts.extend(["o", key]);
        }
        drop(parts);
        Ok(url)
    }
    fn authorization(&self, writer: bool) -> Result<HeaderValue> {
        let file = if writer {
            self.token_file.as_ref().ok_or(REFUSED)?
        } else {
            &self.verifier_file
        };
        let bytes = crate::archive_source::read_private_file(file, 16384).map_err(|_| REFUSED)?;
        let token = std::str::from_utf8(&bytes).map_err(|_| REFUSED)?.trim();
        if token.is_empty()
            || token
                .bytes()
                .any(|c| c.is_ascii_control() || c.is_ascii_whitespace())
        {
            return Err(REFUSED);
        }
        let mut value = HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| REFUSED)?;
        value.set_sensitive(true);
        Ok(value)
    }
    async fn request(
        &self,
        method: Method,
        url: url::Url,
        body: Option<Vec<u8>>,
        writer: bool,
    ) -> Result<Response> {
        let mut request = self
            .http
            .request(method, url)
            .header(reqwest::header::AUTHORIZATION, self.authorization(writer)?);
        if let Some(body) = body {
            request = request
                .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
                .body(body);
        }
        request.send().await.map_err(|_| REFUSED)
    }
    async fn require_permissions(&self, writer: bool) -> Result<()> {
        let mut url = self.url(false, None)?;
        url.path_segments_mut()
            .map_err(|_| REFUSED)?
            .extend(["iam", "testPermissions"]);
        for permission in PERMISSIONS {
            url.query_pairs_mut().append_pair("permissions", permission);
        }
        {
            let response = self.request(Method::GET, url.clone(), None, writer).await?;
            if !response.status().is_success() {
                return Err(REFUSED);
            }
            let response: Value = serde_json::from_slice(&bounded(response, 1024 * 1024).await?)
                .map_err(|_| REFUSED)?;
            let permissions = response["permissions"].as_array().ok_or(REFUSED)?;
            let required = [
                "storage.buckets.get",
                if writer {
                    "storage.objects.create"
                } else {
                    "storage.objects.get"
                },
            ];
            if permissions.len() != 2
                || !required.iter().all(|p| permissions.iter().any(|v| v == p))
            {
                return Err(REFUSED);
            }
        }
        Ok(())
    }
    async fn json(&self, url: url::Url) -> Result<Value> {
        let response = self.request(Method::GET, url, None, false).await?;
        if !response.status().is_success() {
            return Err(REFUSED);
        }
        serde_json::from_slice(&bounded(response, 1024 * 1024).await?).map_err(|_| REFUSED)
    }
    async fn object(&self, key: &str, generation: Option<&str>) -> Result<Value> {
        let mut url = self.url(false, Some(key))?;
        if let Some(generation) = generation {
            url.query_pairs_mut().append_pair("generation", generation);
        }
        self.json(url).await
    }
    fn receipt(
        &self,
        key: String,
        sha256: String,
        size: u64,
        value: &Value,
    ) -> Result<ObjectReceipt> {
        let generation = value["generation"].as_str().ok_or(REFUSED)?;
        if value["bucket"] != self.bucket
            || value["name"] != key
            || value["eventBasedHold"] != true
            || value.get("timeDeleted").is_some()
            || value.get("kmsKeyName").is_some()
            || value.get("customerEncryption").is_some()
            || value["size"].as_str().and_then(|v| v.parse::<u64>().ok()) != Some(size)
            || generation.parse::<u64>().ok().filter(|v| *v > 0).is_none()
            || size > MAX_OBJECT_BYTES as u64
        {
            return Err(REFUSED);
        }
        Ok(ObjectReceipt {
            key,
            generation: generation.into(),
            sha256,
            bytes: size,
        })
    }
}
#[async_trait::async_trait]
impl PermanentCustody for GcsCustody {
    async fn policy(&self) -> Result<CustodyPolicy> {
        let bucket = self.json(self.url(false, None)?).await?;
        if bucket["name"] != self.bucket
            || bucket["labels"]["cma-retirement-operation"] != self.operation
            || bucket["defaultEventBasedHold"] != true
            || bucket["iamConfiguration"]["uniformBucketLevelAccess"]["enabled"] != true
            || bucket["iamConfiguration"]["publicAccessPrevention"] != "enforced"
            || bucket
                .get("encryption")
                .and_then(|v| v.get("defaultKmsKeyName"))
                .is_some()
            || bucket
                .get("lifecycle")
                .and_then(|v| v.get("rule"))
                .is_some_and(|v| v.as_array().is_none_or(|r| !r.is_empty()))
        {
            return Err(REFUSED);
        }
        let metageneration = bucket["metageneration"].as_str().ok_or(REFUSED)?;
        if metageneration
            .parse::<u64>()
            .ok()
            .filter(|v| *v > 0)
            .is_none()
        {
            return Err(REFUSED);
        }
        self.require_permissions(false).await?;
        Ok(CustodyPolicy {
            format: 1,
            bucket: self.bucket.clone(),
            metageneration: metageneration.into(),
            permanent_hold: true,
            private_uniform_access: true,
            independent_read_only: true,
        })
    }
    async fn retain(&self, operation: &str, bytes: Vec<u8>) -> Result<ObjectReceipt> {
        if operation != self.operation || self.token_file.is_none() {
            return Err(REFUSED);
        }
        let key = key(operation, &bytes)?;
        let sha256 = digest(&bytes);
        let size = bytes.len() as u64;
        let policy = self.policy().await?;
        self.require_permissions(true).await?;
        let mut url = self.url(true, None)?;
        url.path_segments_mut().map_err(|_| REFUSED)?.push("o");
        url.query_pairs_mut()
            .append_pair("uploadType", "media")
            .append_pair("name", &key)
            .append_pair("ifGenerationMatch", "0");
        // A lost reply may have committed. Reconcile exact content/hold below;
        // never retry without the generation-zero creation precondition.
        if let Ok(response) = self
            .request(Method::POST, url, Some(bytes.clone()), true)
            .await
        {
            if !response.status().is_success()
                && response.status() != StatusCode::PRECONDITION_FAILED
            {
                return Err(REFUSED);
            }
            bounded(response, 1024 * 1024).await?;
        }
        let object = self.object(&key, None).await?;
        let receipt = self.receipt(key, sha256, size, &object)?;
        if self.read(&receipt).await? != bytes || self.policy().await? != policy {
            return Err(REFUSED);
        }
        Ok(receipt)
    }
    async fn read(&self, receipt: &ObjectReceipt) -> Result<Vec<u8>> {
        if !valid_key(&receipt.key)
            || receipt.key.rsplit('/').next() != Some(receipt.sha256.as_str())
            || receipt
                .generation
                .parse::<u64>()
                .ok()
                .filter(|n| *n > 0)
                .is_none()
            || receipt.bytes > MAX_OBJECT_BYTES as u64
        {
            return Err(REFUSED);
        }
        let metadata = self.receipt(
            receipt.key.clone(),
            receipt.sha256.clone(),
            receipt.bytes,
            &self.object(&receipt.key, Some(&receipt.generation)).await?,
        )?;
        if metadata != *receipt {
            return Err(REFUSED);
        }
        let mut url = self.url(false, Some(&receipt.key))?;
        url.query_pairs_mut()
            .append_pair("generation", &receipt.generation)
            .append_pair("alt", "media");
        let response = self.request(Method::GET, url, None, false).await?;
        if !response.status().is_success() {
            return Err(REFUSED);
        }
        let bytes = bounded(response, receipt.bytes as usize).await?;
        if bytes.len() as u64 != receipt.bytes || digest(&bytes) != receipt.sha256 {
            return Err(REFUSED);
        }
        Ok(bytes)
    }
}
async fn bounded(mut response: Response, limit: usize) -> Result<Vec<u8>> {
    if response.content_length().is_some_and(|n| n > limit as u64) {
        return Err(REFUSED);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| REFUSED)? {
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err(REFUSED);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
