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
//!
//! The object key travels as a query parameter, not a path segment, because
//! that is the contract the storage service exposes. Reads use the dedicated
//! download route: the plain object route returns *metadata* as JSON, and
//! treating that as a transcript would be a success status carrying the wrong
//! bytes — the one failure mode here that would not announce itself.

use layer2_core::{
    store::{ContextStore, StoreError},
    SessionId,
};

/// Content type recorded for a stored transcript.
///
/// The transcript is newline-delimited JSON, and saying so means anything that
/// later inspects the bucket sees what it is rather than opaque bytes.
const CONTENT_TYPE: &str = "application/x-ndjson";

/// Object store for session context, via the platform's storage service.
pub struct ChronoStorageContextStore {
    /// Fully qualified base, already including the proxy path to the service.
    base_url: String,
    /// Bucket that holds this deployment's session context.
    bucket: String,
    /// Credential presented to the identity platform.
    ///
    /// Belongs to the platform's trust domain and nothing else. It is passed in
    /// rather than derived so that a credential issued for another service can
    /// never be spent here by accident.
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

    /// The collection endpoint, which every operation addresses by key.
    fn objects_url(&self) -> String {
        format!("{}/api/buckets/{}/objects", self.base_url, self.bucket)
    }

    /// The endpoint that streams an object's raw bytes.
    fn download_url(&self) -> String {
        format!("{}/download", self.objects_url())
    }
}

#[async_trait::async_trait]
impl ContextStore for ChronoStorageContextStore {
    async fn put(&self, id: &SessionId, bytes: Vec<u8>) -> Result<String, StoreError> {
        let key = Self::key_for(id);
        let url = self.objects_url();
        let len = bytes.len();
        // A create is an overwrite on this service, which suits a transcript
        // that is always written whole rather than appended to.
        let resp = self
            .http
            .post(&url)
            .query(&[("key", key.as_str()), ("contentType", CONTENT_TYPE)])
            .bearer_auth(&self.credential)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .body(bytes)
            .send()
            .await
            .map_err(|e| StoreError::Unavailable(format!("POST {url}: {e}")))?;
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
        let url = self.download_url();
        let resp = self
            .http
            .get(&url)
            .query(&[("key", reference)])
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
        let url = self.objects_url();
        let resp = self
            .http
            .delete(&url)
            .query(&[("key", reference)])
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
            store().objects_url(),
            "http://platform/api/v1/proxy/s/storage/api/buckets/bkt/objects"
        );
    }

    #[test]
    fn the_key_is_never_a_path_segment() {
        // The service takes the key as a query parameter. Building it into the
        // path is the mistake this asserts against: it produced 404s on write
        // and delete, and the endpoint it did reach on read returns metadata.
        let s = store();
        assert!(
            !s.objects_url().contains("context.jsonl"),
            "the key must not appear in the path"
        );
        assert!(s.objects_url().ends_with("/objects"));
    }

    #[test]
    fn reads_use_the_download_route_not_the_metadata_route() {
        // The plain object route answers with metadata JSON. Reading a
        // transcript from it would succeed and return the wrong bytes, which is
        // worse than failing.
        assert_eq!(
            store().download_url(),
            "http://platform/api/v1/proxy/s/storage/api/buckets/bkt/objects/download"
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
    async fn an_unreachable_store_fails_a_read_rather_than_reporting_no_context() {
        // Reporting None here would tell a reviving session it had no history
        // when it simply could not be reached.
        let s = ChronoStorageContextStore::new("http://127.0.0.1:1", "b", "t");
        assert!(s.get("sessions/a/context.jsonl").await.is_err());
    }

    #[tokio::test]
    async fn deleting_against_an_unreachable_store_is_reported() {
        let s = ChronoStorageContextStore::new("http://127.0.0.1:1", "b", "t");
        assert!(s.delete("sessions/gone/context.jsonl").await.is_err());
    }
}
