//! The agent's accumulated context, kept in the platform object store.
//!
//! A session's context is the transcript the agent rebuilds itself from. It
//! grows for as long as the conversation does, is written and read whole, and
//! would eventually outgrow a document store — so it goes to object storage
//! rather than beside the session record.
//!
//! Reached through the identity platform's proxy, the same way every other
//! service on this platform is reached, so the credential is brokered rather
//! than held here.

use layer2_core::{
    store::{ContextStore, StoreError},
    SessionId,
};

/// Object store for session context, via the platform's storage service.
pub struct ChronoStorageContextStore {
    /// Fully qualified base, already including the proxy path to the service.
    base_url: String,
    /// Bucket that holds this deployment's session context.
    bucket: String,
    /// Credential presented to the platform.
    credential: String,
    http: reqwest::Client,
}

impl ChronoStorageContextStore {
    /// Build a context store.
    pub fn new(
        base_url: impl Into<String>,
        bucket: impl Into<String>,
        credential: impl Into<String>,
    ) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            bucket: bucket.into(),
            credential: credential.into(),
            http: reqwest::Client::builder()
                // Context uploads are large and grow with the conversation.
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .expect("http client builds"),
        }
    }

    /// Object key for a session's context.
    ///
    /// Derived from the session id rather than stored, so a lost reference can
    /// always be recomputed and an orphan is identifiable by name alone.
    pub fn key_for(id: &SessionId) -> String {
        format!("sessions/{}/context.jsonl", id.0)
    }

    fn object_url(&self, key: &str) -> String {
        format!(
            "{}/api/buckets/{}/objects/{}",
            self.base_url, self.bucket, key
        )
    }
}

#[async_trait::async_trait]
impl ContextStore for ChronoStorageContextStore {
    async fn put(&self, id: &SessionId, bytes: Vec<u8>) -> Result<String, StoreError> {
        let key = Self::key_for(id);
        let url = self.object_url(&key);
        let len = bytes.len();
        let resp = self
            .http
            .put(&url)
            .bearer_auth(&self.credential)
            .header(reqwest::header::CONTENT_TYPE, "application/x-ndjson")
            .body(bytes)
            .send()
            .await
            .map_err(|e| StoreError::Unavailable(format!("PUT {url}: {e}")))?;
        if !resp.status().is_success() {
            return Err(StoreError::Unavailable(format!(
                "context upload returned {}",
                resp.status()
            )));
        }
        tracing::info!(session_id = %id, bytes = len, "session context stored");
        Ok(key)
    }

    async fn get(&self, reference: &str) -> Result<Option<Vec<u8>>, StoreError> {
        let url = self.object_url(reference);
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&self.credential)
            .send()
            .await
            .map_err(|e| StoreError::Unavailable(format!("GET {url}: {e}")))?;
        if resp.status().as_u16() == 404 {
            // A session that never produced context is not an error; it simply
            // has nothing to restore.
            return Ok(None);
        }
        if !resp.status().is_success() {
            return Err(StoreError::Unavailable(format!(
                "context download returned {}",
                resp.status()
            )));
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| StoreError::Unavailable(format!("context body: {e}")))?;
        Ok(Some(bytes.to_vec()))
    }

    async fn delete(&self, reference: &str) -> Result<(), StoreError> {
        let url = self.object_url(reference);
        let resp = self
            .http
            .delete(&url)
            .bearer_auth(&self.credential)
            .send()
            .await
            .map_err(|e| StoreError::Unavailable(format!("DELETE {url}: {e}")))?;
        let status = resp.status();
        if status.is_success() || status.as_u16() == 404 {
            return Ok(());
        }
        Err(StoreError::Unavailable(format!(
            "context delete returned {status}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> ChronoStorageContextStore {
        ChronoStorageContextStore::new("http://platform/api/v1/proxy/s/storage/", "bkt", "tok")
    }

    #[test]
    fn the_key_is_derived_from_the_session_id() {
        // Derivable rather than stored: a lost reference can be recomputed, and
        // an orphaned object is identifiable from its name alone.
        assert_eq!(
            ChronoStorageContextStore::key_for(&SessionId("sesn_1".into())),
            "sessions/sesn_1/context.jsonl"
        );
    }

    #[test]
    fn a_trailing_slash_on_the_base_does_not_double_up() {
        assert_eq!(
            store().object_url("sessions/a/context.jsonl"),
            "http://platform/api/v1/proxy/s/storage/api/buckets/bkt/objects/sessions/a/context.jsonl"
        );
    }

    #[tokio::test]
    async fn an_unreachable_store_reports_unavailable_rather_than_losing_data() {
        // Port 1 is reserved and never listening, so this exercises the failure
        // path deterministically.
        let s = ChronoStorageContextStore::new("http://127.0.0.1:1", "b", "t");
        let err = s
            .put(&SessionId("sesn_1".into()), b"x".to_vec())
            .await
            .expect_err("must fail against a dead store");
        assert!(matches!(err, StoreError::Unavailable(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn deleting_something_already_gone_is_not_an_error() {
        // Reconciliation deletes optimistically; an object already reaped by
        // another pass must not fail the sweep.
        let s = ChronoStorageContextStore::new("http://127.0.0.1:1", "b", "t");
        // Unreachable still errors; the 404 tolerance is exercised where a real
        // status is available.
        assert!(s.delete("sessions/gone/context.jsonl").await.is_err());
    }
}
