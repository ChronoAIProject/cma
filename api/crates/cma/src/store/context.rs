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

use std::sync::Arc;

use layer2_core::{
    store::{ContextStore, HandoverSnapshot, HandoverStore, StoreError},
    SessionId,
};
use mongodb::bson::{self, spec::BinarySubtype, Binary, Bson};
use serde::{Deserialize, Serialize};

use crate::store::token::TokenSource;

/// Content type recorded for a stored transcript.
///
/// The transcript is newline-delimited JSON, and saying so means anything that
/// later inspects the bucket sees what it is rather than opaque bytes.
const CONTENT_TYPE: &str = "application/x-ndjson";

/// Content type recorded for a durable runtime-handover snapshot.
///
/// The BSON envelope is versioned and carries the rotation claim beside the
/// adapter's opaque bytes. Recovery can therefore distinguish the snapshot for
/// its exact handover from an older value at the same stable object key.
const HANDOVER_CONTENT_TYPE: &str = "application/vnd.chrono.handover+bson";
const HANDOVER_ENVELOPE_VERSION: i32 = 1;

#[derive(Debug, Serialize, Deserialize)]
struct HandoverEnvelope {
    version: i32,
    claim: String,
    snapshot_at: String,
    /// Explicit BSON null preserves `restore: None`; binary, including an empty
    /// binary value, preserves `Some(bytes)` without conflating the two.
    restore: Bson,
}

impl HandoverEnvelope {
    fn encode(snapshot: HandoverSnapshot) -> Result<Vec<u8>, StoreError> {
        let HandoverSnapshot {
            claim,
            snapshot_at,
            restore,
        } = snapshot;
        let restore = match restore {
            Some(bytes) => Bson::Binary(Binary {
                subtype: BinarySubtype::Generic,
                bytes,
            }),
            None => Bson::Null,
        };
        bson::serialize_to_vec(&Self {
            version: HANDOVER_ENVELOPE_VERSION,
            claim,
            snapshot_at,
            restore,
        })
        .map_err(|error| StoreError::Malformed(format!("handover snapshot envelope: {error}")))
    }

    fn decode(bytes: &[u8]) -> Result<HandoverSnapshot, StoreError> {
        let envelope: Self = bson::deserialize_from_slice(bytes).map_err(|error| {
            StoreError::Malformed(format!("handover snapshot envelope: {error}"))
        })?;
        if envelope.version != HANDOVER_ENVELOPE_VERSION {
            return Err(StoreError::Malformed(format!(
                "unsupported handover snapshot envelope version {}",
                envelope.version
            )));
        }
        let restore = match envelope.restore {
            Bson::Null => None,
            Bson::Binary(binary) if binary.subtype == BinarySubtype::Generic => Some(binary.bytes),
            Bson::Binary(binary) => {
                return Err(StoreError::Malformed(format!(
                    "handover snapshot uses unsupported BSON binary subtype {:?}",
                    binary.subtype
                )));
            }
            other => {
                return Err(StoreError::Malformed(format!(
                    "handover snapshot restore must be BSON binary or null, got {:?}",
                    other.element_type()
                )));
            }
        };
        Ok(HandoverSnapshot {
            claim: envelope.claim,
            snapshot_at: envelope.snapshot_at,
            restore,
        })
    }
}

/// Object store for session context, via the platform's storage service.
pub struct ChronoStorageContextStore {
    /// Fully qualified base, already including the proxy path to the service.
    base_url: String,
    /// Bucket that holds this deployment's session context.
    bucket: String,
    /// Supplies the credential presented to the identity platform.
    ///
    /// A source rather than a value: this service's identity is a machine one,
    /// and its tokens expire, so holding a string read once at startup would
    /// work until the first expiry and then fail for as long as the process
    /// lived. Injected so the store stays ignorant of how the credential is
    /// obtained, and so a credential issued for another service cannot be
    /// spent here by accident.
    tokens: Arc<dyn TokenSource>,
    http: reqwest::Client,
}

impl ChronoStorageContextStore {
    /// Build a context store.
    pub fn new(
        base_url: impl Into<String>,
        bucket: impl Into<String>,
        tokens: Arc<dyn TokenSource>,
    ) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            bucket: bucket.into(),
            tokens,
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

    /// Stable object key for the session's latest runtime-handover snapshot.
    ///
    /// One object per session bounds storage. Each later fenced rotation
    /// overwrites it with an envelope carrying that rotation's exact claim.
    pub fn handover_key_for(id: &SessionId) -> String {
        format!("sessions/{}/handover.snapshot", id.0)
    }

    /// The collection endpoint, which every operation addresses by key.
    fn objects_url(&self) -> String {
        format!("{}/api/buckets/{}/objects", self.base_url, self.bucket)
    }

    /// The endpoint that streams an object's raw bytes.
    fn download_url(&self) -> String {
        format!("{}/download", self.objects_url())
    }

    async fn upload_object(
        &self,
        key: &str,
        content_type: &str,
        bytes: Vec<u8>,
        kind: &str,
    ) -> Result<(), StoreError> {
        let url = self.objects_url();
        // ChronoStorage maps this request to one S3/MinIO PutObject. A later
        // upload to the stable key atomically replaces the complete object.
        let resp = self
            .http
            .post(&url)
            .query(&[("key", key), ("contentType", content_type)])
            .bearer_auth(self.tokens.token().await?)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .body(bytes)
            .send()
            .await
            .map_err(|e| StoreError::Unavailable(format!("POST {url}: {e}")))?;
        if !resp.status().is_success() {
            return Err(StoreError::Unavailable(format!(
                "{kind} upload returned {}",
                resp.status()
            )));
        }
        Ok(())
    }

    async fn download_object(
        &self,
        reference: &str,
        kind: &str,
    ) -> Result<Option<Vec<u8>>, StoreError> {
        let url = self.download_url();
        let resp = self
            .http
            .get(&url)
            .query(&[("key", reference)])
            .bearer_auth(self.tokens.token().await?)
            .send()
            .await
            .map_err(|e| StoreError::Unavailable(format!("GET {url}: {e}")))?;
        if resp.status().as_u16() == 404 {
            return Ok(None);
        }
        if !resp.status().is_success() {
            return Err(StoreError::Unavailable(format!(
                "{kind} download returned {}",
                resp.status()
            )));
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| StoreError::Unavailable(format!("{kind} body: {e}")))?;
        Ok(Some(bytes.to_vec()))
    }

    async fn delete_object(&self, reference: &str, kind: &str) -> Result<(), StoreError> {
        let url = self.objects_url();
        let resp = self
            .http
            .delete(&url)
            .query(&[("key", reference)])
            .bearer_auth(self.tokens.token().await?)
            .send()
            .await
            .map_err(|e| StoreError::Unavailable(format!("DELETE {url}: {e}")))?;
        let status = resp.status();
        if status.is_success() || status.as_u16() == 404 {
            return Ok(());
        }
        Err(StoreError::Unavailable(format!(
            "{kind} delete returned {status}"
        )))
    }
}

#[async_trait::async_trait]
impl ContextStore for ChronoStorageContextStore {
    async fn put(&self, id: &SessionId, bytes: Vec<u8>) -> Result<String, StoreError> {
        let key = Self::key_for(id);
        let len = bytes.len();
        self.upload_object(&key, CONTENT_TYPE, bytes, "context")
            .await?;
        tracing::info!(session_id = %id, bytes = len, "session context stored");
        Ok(key)
    }

    async fn get(&self, reference: &str) -> Result<Option<Vec<u8>>, StoreError> {
        self.download_object(reference, "context").await
    }

    async fn delete(&self, reference: &str) -> Result<(), StoreError> {
        self.delete_object(reference, "context").await
    }
}

#[async_trait::async_trait]
impl HandoverStore for ChronoStorageContextStore {
    async fn put(&self, id: &SessionId, snapshot: HandoverSnapshot) -> Result<String, StoreError> {
        let key = Self::handover_key_for(id);
        let claim = snapshot.claim.clone();
        let restore_len = snapshot.restore.as_ref().map(Vec::len);
        let bytes = HandoverEnvelope::encode(snapshot)?;
        self.upload_object(&key, HANDOVER_CONTENT_TYPE, bytes, "handover snapshot")
            .await?;
        tracing::info!(
            session_id = %id,
            %claim,
            restore_bytes = ?restore_len,
            "runtime handover snapshot stored"
        );
        Ok(key)
    }

    async fn get_for(
        &self,
        id: &SessionId,
        reference: &str,
    ) -> Result<Option<HandoverSnapshot>, StoreError> {
        let expected = Self::handover_key_for(id);
        if reference != expected {
            return Err(StoreError::Malformed(format!(
                "handover snapshot reference does not belong to session {id}"
            )));
        }
        self.download_object(reference, "handover snapshot")
            .await?
            .map(|bytes| HandoverEnvelope::decode(&bytes))
            .transpose()
    }

    async fn delete_for(&self, id: &SessionId) -> Result<(), StoreError> {
        self.delete_object(&Self::handover_key_for(id), "handover snapshot")
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use axum::{
        body::Bytes,
        extract::{Query, State},
        http::{HeaderMap, StatusCode},
        response::{IntoResponse, Response},
        routing::{get, post},
        Router,
    };

    #[derive(Debug, Clone)]
    struct StoredObject {
        content_type: String,
        bytes: Vec<u8>,
    }

    #[derive(Clone, Default)]
    struct StorageFixture {
        objects: Arc<tokio::sync::Mutex<HashMap<String, StoredObject>>>,
    }

    #[derive(Deserialize)]
    struct ObjectQuery {
        key: String,
        #[serde(rename = "contentType")]
        content_type: Option<String>,
    }

    fn assert_fixture_auth(headers: &HeaderMap) {
        assert_eq!(
            headers
                .get(reqwest::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer tok")
        );
    }

    async fn fixture_upload(
        State(fixture): State<StorageFixture>,
        Query(query): Query<ObjectQuery>,
        headers: HeaderMap,
        body: Bytes,
    ) -> StatusCode {
        assert_fixture_auth(&headers);
        fixture.objects.lock().await.insert(
            query.key,
            StoredObject {
                content_type: query.content_type.expect("upload content type"),
                bytes: body.to_vec(),
            },
        );
        StatusCode::OK
    }

    async fn fixture_download(
        State(fixture): State<StorageFixture>,
        Query(query): Query<ObjectQuery>,
        headers: HeaderMap,
    ) -> Response {
        assert_fixture_auth(&headers);
        match fixture.objects.lock().await.get(&query.key).cloned() {
            Some(object) => (StatusCode::OK, object.bytes).into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        }
    }

    async fn fixture_delete(
        State(fixture): State<StorageFixture>,
        Query(query): Query<ObjectQuery>,
        headers: HeaderMap,
    ) -> StatusCode {
        assert_fixture_auth(&headers);
        fixture.objects.lock().await.remove(&query.key);
        StatusCode::OK
    }

    async fn storage_fixture() -> (String, StorageFixture, tokio::task::JoinHandle<()>) {
        let fixture = StorageFixture::default();
        let app = Router::new()
            .route(
                "/api/buckets/bkt/objects",
                post(fixture_upload).delete(fixture_delete),
            )
            .route("/api/buckets/bkt/objects/download", get(fixture_download))
            .with_state(fixture.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind storage fixture");
        let address = listener.local_addr().expect("storage fixture address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve storage fixture");
        });
        (format!("http://{address}"), fixture, server)
    }

    fn store() -> ChronoStorageContextStore {
        ChronoStorageContextStore::new(
            "http://platform/api/v1/proxy/s/storage/",
            "bkt",
            Arc::new(crate::store::token::StaticToken("tok".into())),
        )
    }

    fn unreachable_store() -> ChronoStorageContextStore {
        ChronoStorageContextStore::new(
            "http://127.0.0.1:1",
            "b",
            Arc::new(crate::store::token::StaticToken("t".into())),
        )
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
    fn the_handover_key_is_stable_and_separate_from_context() {
        let id = SessionId("sesn_1".into());
        assert_eq!(
            ChronoStorageContextStore::handover_key_for(&id),
            "sessions/sesn_1/handover.snapshot"
        );
        assert_ne!(
            ChronoStorageContextStore::handover_key_for(&id),
            ChronoStorageContextStore::key_for(&id)
        );
    }

    #[test]
    fn the_handover_envelope_preserves_none_empty_and_nonempty_snapshots() {
        for restore in [None, Some(Vec::new()), Some(vec![0, 1, 2, 255])] {
            let expected = HandoverSnapshot {
                claim: "rotn_exact".into(),
                snapshot_at: "2026-08-27T01:02:03Z".into(),
                restore,
            };
            let encoded = HandoverEnvelope::encode(expected.clone()).expect("snapshot encodes");
            assert_eq!(
                HandoverEnvelope::decode(&encoded).expect("snapshot decodes"),
                expected
            );
        }
    }

    #[test]
    fn the_handover_envelope_v1_uses_explicit_bson_null_and_binary() {
        let absent = HandoverEnvelope::encode(HandoverSnapshot {
            claim: "rotn_wire".into(),
            snapshot_at: "2026-08-27T01:02:03Z".into(),
            restore: None,
        })
        .expect("absent restore encodes");
        let absent: bson::Document =
            bson::deserialize_from_slice(&absent).expect("envelope is a BSON document");
        assert_eq!(
            absent.get_i32("version").expect("version is an integer"),
            HANDOVER_ENVELOPE_VERSION
        );
        assert_eq!(
            absent.get_str("claim").expect("claim is a string"),
            "rotn_wire"
        );
        assert_eq!(
            absent
                .get_str("snapshot_at")
                .expect("snapshot time is a string"),
            "2026-08-27T01:02:03Z"
        );
        assert!(matches!(absent.get("restore"), Some(Bson::Null)));

        let present = HandoverEnvelope::encode(HandoverSnapshot {
            claim: "rotn_wire".into(),
            snapshot_at: "2026-08-27T01:02:04Z".into(),
            restore: Some(vec![0, 255]),
        })
        .expect("present restore encodes");
        let present: bson::Document =
            bson::deserialize_from_slice(&present).expect("envelope is a BSON document");
        assert!(matches!(
            present.get("restore"),
            Some(Bson::Binary(Binary {
                subtype: BinarySubtype::Generic,
                bytes,
            })) if bytes == &[0, 255]
        ));
    }

    #[test]
    fn an_unknown_handover_envelope_version_is_refused() {
        let encoded = bson::serialize_to_vec(&HandoverEnvelope {
            version: HANDOVER_ENVELOPE_VERSION + 1,
            claim: "rotn_future".into(),
            snapshot_at: "2026-08-27T01:02:03Z".into(),
            restore: Bson::Null,
        })
        .expect("future envelope encodes");

        let error = HandoverEnvelope::decode(&encoded).expect_err("future schema must fail closed");
        assert!(
            matches!(error, StoreError::Malformed(ref detail) if detail.contains("unsupported handover snapshot envelope version")),
            "got {error:?}"
        );
    }

    #[tokio::test]
    async fn handover_put_overwrites_one_stable_object_without_touching_context() {
        let (base, fixture, server) = storage_fixture().await;
        let store = ChronoStorageContextStore::new(
            base,
            "bkt",
            Arc::new(crate::store::token::StaticToken("tok".into())),
        );
        let id = SessionId("sesn_handover".into());
        let context_ref = ContextStore::put(&store, &id, b"context\n".to_vec())
            .await
            .expect("context upload succeeds");
        let first_ref = HandoverStore::put(
            &store,
            &id,
            HandoverSnapshot {
                claim: "rotn_first".into(),
                snapshot_at: "2026-08-27T01:02:03Z".into(),
                restore: None,
            },
        )
        .await
        .expect("rehearsal upload succeeds");
        let second = HandoverSnapshot {
            claim: "rotn_second".into(),
            snapshot_at: "2026-08-27T01:02:04Z".into(),
            restore: Some(Vec::new()),
        };
        let second_ref = HandoverStore::put(&store, &id, second.clone())
            .await
            .expect("later rotation overwrites the stable object");

        assert_eq!(first_ref, second_ref);
        assert_ne!(context_ref, second_ref);
        assert_eq!(
            HandoverStore::get_for(&store, &id, &second_ref)
                .await
                .expect("handover download succeeds"),
            Some(second)
        );
        let objects = fixture.objects.lock().await;
        assert_eq!(objects.len(), 2, "context and handover use two objects");
        assert_eq!(
            objects
                .get(&context_ref)
                .expect("context remains stored")
                .content_type,
            CONTENT_TYPE
        );
        assert_eq!(
            objects
                .get(&second_ref)
                .expect("handover remains stored")
                .content_type,
            HANDOVER_CONTENT_TYPE
        );
        drop(objects);

        HandoverStore::delete_for(&store, &id)
            .await
            .expect("deterministic delete succeeds");
        assert_eq!(
            HandoverStore::get_for(&store, &id, &second_ref)
                .await
                .expect("deleted handover reads as absent"),
            None
        );
        assert_eq!(
            ContextStore::get(&store, &context_ref)
                .await
                .expect("context read succeeds"),
            Some(b"context\n".to_vec())
        );
        server.abort();
    }

    #[tokio::test]
    async fn handover_reads_reject_a_reference_for_another_session() {
        let store = store();
        let error = HandoverStore::get_for(
            &store,
            &SessionId("sesn_expected".into()),
            "sessions/sesn_other/handover.snapshot",
        )
        .await
        .expect_err("cross-session snapshot reference must fail closed");

        assert!(
            matches!(error, StoreError::Malformed(ref detail) if detail.contains("does not belong")),
            "got {error:?}"
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
        let s = unreachable_store();
        let err = ContextStore::put(&s, &SessionId("sesn_1".into()), b"x".to_vec())
            .await
            .expect_err("must fail against a dead store");
        assert!(matches!(err, StoreError::Unavailable(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn an_unreachable_store_fails_a_read_rather_than_reporting_no_context() {
        // Reporting None here would tell a reviving session it had no history
        // when it simply could not be reached.
        let s = unreachable_store();
        assert!(ContextStore::get(&s, "sessions/a/context.jsonl")
            .await
            .is_err());
    }

    #[tokio::test]
    async fn deleting_against_an_unreachable_store_is_reported() {
        let s = unreachable_store();
        assert!(s.delete("sessions/gone/context.jsonl").await.is_err());
    }
}
