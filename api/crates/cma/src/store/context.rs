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
    store::{
        ChatCheckpointObjectReceipt, ChatCheckpointStore, ContextStore, HandoverSnapshot,
        HandoverStore, StoreError,
    },
    ChatHostId, SessionId,
};
use mongodb::bson::{self, spec::BinarySubtype, Binary, Bson};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

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
const CHAT_CHECKPOINT_CONTENT_TYPE: &str = "application/vnd.chrono.chat-checkpoint+bson";
const CHAT_CHECKPOINT_ENVELOPE_VERSION: i32 = 1;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChatCheckpointEnvelope {
    version: i32,
    generations: Vec<ChatCheckpointGeneration>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChatCheckpointGeneration {
    sha256: String,
    bytes: i64,
    payload: Binary,
}

impl ChatCheckpointEnvelope {
    fn encode(generations: Vec<ChatCheckpointGeneration>) -> Result<Vec<u8>, StoreError> {
        let envelope = Self {
            version: CHAT_CHECKPOINT_ENVELOPE_VERSION,
            generations,
        };
        envelope.validate()?;
        bson::serialize_to_vec(&envelope)
            .map_err(|error| StoreError::Malformed(format!("chat checkpoint envelope: {error}")))
    }

    fn decode(bytes: &[u8]) -> Result<Self, StoreError> {
        let envelope: Self = bson::deserialize_from_slice(bytes)
            .map_err(|error| StoreError::Malformed(format!("chat checkpoint envelope: {error}")))?;
        envelope.validate()?;
        Ok(envelope)
    }

    fn validate(&self) -> Result<(), StoreError> {
        if self.version != CHAT_CHECKPOINT_ENVELOPE_VERSION {
            return Err(StoreError::Malformed(format!(
                "unsupported chat checkpoint envelope version {}",
                self.version
            )));
        }
        if self.generations.is_empty() || self.generations.len() > 2 {
            return Err(StoreError::Malformed(
                "a chat checkpoint envelope must retain one or two generations".into(),
            ));
        }
        let mut digests = std::collections::BTreeSet::new();
        for generation in &self.generations {
            require_checkpoint_sha256(&generation.sha256)?;
            if generation.payload.subtype != BinarySubtype::Generic
                || generation.payload.bytes.is_empty()
                || usize::try_from(generation.bytes).ok() != Some(generation.payload.bytes.len())
                || checkpoint_digest(&generation.payload.bytes) != generation.sha256
                || !digests.insert(generation.sha256.as_str())
            {
                return Err(StoreError::Malformed(
                    "a chat checkpoint generation does not match its immutable identity".into(),
                ));
            }
        }
        Ok(())
    }

    fn generation(&self, sha256: &str) -> Option<&ChatCheckpointGeneration> {
        self.generations
            .iter()
            .find(|generation| generation.sha256 == sha256)
    }
}

impl ChatCheckpointGeneration {
    fn new(payload: Vec<u8>) -> Result<Self, StoreError> {
        if payload.is_empty() {
            return Err(StoreError::Malformed(
                "a chat checkpoint object must not be empty".into(),
            ));
        }
        let bytes = i64::try_from(payload.len()).map_err(|_| {
            StoreError::Malformed("a chat checkpoint object exceeds the storage wire bound".into())
        })?;
        Ok(Self {
            sha256: checkpoint_digest(&payload),
            bytes,
            payload: Binary {
                subtype: BinarySubtype::Generic,
                bytes: payload,
            },
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

    /// Stable physical key carrying at most two logical conversation generations.
    pub fn chat_conversation_checkpoint_key_for(id: &SessionId) -> String {
        Self::handover_key_for(id)
    }

    /// Stable physical key carrying at most two logical workspace generations.
    pub fn chat_workspace_checkpoint_key_for(host: &ChatHostId) -> String {
        format!("chat-hosts/{}/checkpoint-v1", host.0)
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

    async fn stage_chat_checkpoint_object(
        &self,
        key: &str,
        retain_sha256: Option<&str>,
        payload: Vec<u8>,
        kind: &str,
    ) -> Result<ChatCheckpointObjectReceipt, StoreError> {
        let candidate = ChatCheckpointGeneration::new(payload)?;
        let retained = match retain_sha256 {
            Some(retain_sha256) => {
                require_checkpoint_sha256(retain_sha256)?;
                let bytes = self.download_object(key, kind).await?.ok_or_else(|| {
                    StoreError::Unavailable(format!("the committed {kind} generation is missing"))
                })?;
                let envelope = ChatCheckpointEnvelope::decode(&bytes)?;
                Some(envelope.generation(retain_sha256).cloned().ok_or_else(|| {
                    StoreError::Unavailable(format!(
                        "the committed {kind} generation is no longer retained"
                    ))
                })?)
            }
            None => None,
        };
        let mut generations = retained.into_iter().collect::<Vec<_>>();
        if generations
            .first()
            .is_none_or(|retained| retained.sha256 != candidate.sha256)
        {
            generations.push(candidate.clone());
        }
        let encoded = ChatCheckpointEnvelope::encode(generations)?;
        self.upload_object(key, CHAT_CHECKPOINT_CONTENT_TYPE, encoded, kind)
            .await?;

        let readback = self
            .download_object(key, kind)
            .await?
            .ok_or_else(|| StoreError::Unavailable(format!("the staged {kind} is missing")))?;
        let readback = ChatCheckpointEnvelope::decode(&readback)?;
        let verified = readback.generation(&candidate.sha256).ok_or_else(|| {
            StoreError::Unavailable(format!("the staged {kind} generation was not retained"))
        })?;
        Ok(ChatCheckpointObjectReceipt {
            object_reference: key.to_string(),
            sha256: verified.sha256.clone(),
            bytes: u64::try_from(verified.bytes).map_err(|_| {
                StoreError::Malformed(format!("the staged {kind} has a negative size"))
            })?,
        })
    }

    async fn read_chat_checkpoint_object(
        &self,
        key: &str,
        sha256: &str,
        kind: &str,
    ) -> Result<Option<Vec<u8>>, StoreError> {
        require_checkpoint_sha256(sha256)?;
        let Some(bytes) = self.download_object(key, kind).await? else {
            return Ok(None);
        };
        let envelope = ChatCheckpointEnvelope::decode(&bytes)?;
        Ok(envelope
            .generation(sha256)
            .map(|generation| generation.payload.bytes.clone()))
    }

    async fn prune_chat_checkpoint_object(
        &self,
        key: &str,
        keep_sha256: &str,
        kind: &str,
    ) -> Result<(), StoreError> {
        require_checkpoint_sha256(keep_sha256)?;
        let bytes = self.download_object(key, kind).await?.ok_or_else(|| {
            StoreError::Unavailable(format!("the committed {kind} object is missing"))
        })?;
        let envelope = ChatCheckpointEnvelope::decode(&bytes)?;
        let retained = envelope.generation(keep_sha256).cloned().ok_or_else(|| {
            StoreError::Unavailable(format!(
                "the committed {kind} generation is no longer retained"
            ))
        })?;
        if envelope.generations.len() == 1 {
            return Ok(());
        }
        let encoded = ChatCheckpointEnvelope::encode(vec![retained])?;
        self.upload_object(key, CHAT_CHECKPOINT_CONTENT_TYPE, encoded, kind)
            .await
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

#[async_trait::async_trait]
impl ChatCheckpointStore for ChronoStorageContextStore {
    async fn stage_conversation(
        &self,
        id: &SessionId,
        retain_sha256: Option<&str>,
        snapshot: Vec<u8>,
    ) -> Result<ChatCheckpointObjectReceipt, StoreError> {
        let key = Self::chat_conversation_checkpoint_key_for(id);
        self.stage_chat_checkpoint_object(
            &key,
            retain_sha256,
            snapshot,
            "chat conversation checkpoint",
        )
        .await
    }

    async fn read_conversation(
        &self,
        id: &SessionId,
        object_reference: &str,
        sha256: &str,
    ) -> Result<Option<Vec<u8>>, StoreError> {
        let expected = Self::chat_conversation_checkpoint_key_for(id);
        if object_reference != expected {
            return Err(StoreError::Malformed(format!(
                "chat checkpoint reference does not belong to session {id}"
            )));
        }
        self.read_chat_checkpoint_object(object_reference, sha256, "chat conversation checkpoint")
            .await
    }

    async fn prune_conversation(
        &self,
        id: &SessionId,
        keep_sha256: &str,
    ) -> Result<(), StoreError> {
        self.prune_chat_checkpoint_object(
            &Self::chat_conversation_checkpoint_key_for(id),
            keep_sha256,
            "chat conversation checkpoint",
        )
        .await
    }

    async fn stage_workspace(
        &self,
        host: &ChatHostId,
        retain_sha256: Option<&str>,
        archive: Vec<u8>,
    ) -> Result<ChatCheckpointObjectReceipt, StoreError> {
        let key = Self::chat_workspace_checkpoint_key_for(host);
        self.stage_chat_checkpoint_object(&key, retain_sha256, archive, "chat workspace checkpoint")
            .await
    }

    async fn read_workspace(
        &self,
        host: &ChatHostId,
        object_reference: &str,
        sha256: &str,
    ) -> Result<Option<Vec<u8>>, StoreError> {
        let expected = Self::chat_workspace_checkpoint_key_for(host);
        if object_reference != expected {
            return Err(StoreError::Malformed(format!(
                "chat workspace checkpoint reference does not belong to host {host}"
            )));
        }
        self.read_chat_checkpoint_object(object_reference, sha256, "chat workspace checkpoint")
            .await
    }

    async fn prune_workspace(
        &self,
        host: &ChatHostId,
        keep_sha256: &str,
    ) -> Result<(), StoreError> {
        self.prune_chat_checkpoint_object(
            &Self::chat_workspace_checkpoint_key_for(host),
            keep_sha256,
            "chat workspace checkpoint",
        )
        .await
    }
}

fn require_checkpoint_sha256(value: &str) -> Result<(), StoreError> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(StoreError::Malformed(
            "chat checkpoint digest must be a SHA-256".into(),
        ));
    }
    Ok(())
}

fn checkpoint_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    const CHARS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push(char::from(CHARS[usize::from(byte >> 4)]));
        out.push(char::from(CHARS[usize::from(byte & 0x0f)]));
    }
    out
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
    fn chat_checkpoint_keys_are_stable_and_partitioned_by_owner() {
        let session = SessionId("sesn_chat_1".into());
        let host = ChatHostId("chho_owner_1".into());
        assert_eq!(
            ChronoStorageContextStore::chat_conversation_checkpoint_key_for(&session),
            "sessions/sesn_chat_1/handover.snapshot"
        );
        assert_eq!(
            ChronoStorageContextStore::chat_workspace_checkpoint_key_for(&host),
            "chat-hosts/chho_owner_1/checkpoint-v1"
        );
    }

    #[test]
    fn chat_checkpoint_envelopes_reject_tampered_and_ambiguous_generations() {
        let first = ChatCheckpointGeneration::new(b"first".to_vec()).unwrap();
        let second = ChatCheckpointGeneration::new(b"second".to_vec()).unwrap();
        let encoded = ChatCheckpointEnvelope::encode(vec![first.clone(), second.clone()]).unwrap();
        let decoded = ChatCheckpointEnvelope::decode(&encoded).unwrap();
        assert_eq!(decoded.generations.len(), 2);
        let retained = decoded.generation(&first.sha256).unwrap();
        assert_eq!(retained.sha256, first.sha256);
        assert_eq!(retained.bytes, first.bytes);
        assert_eq!(retained.payload, first.payload);

        for malformed in [
            ChatCheckpointEnvelope {
                version: CHAT_CHECKPOINT_ENVELOPE_VERSION + 1,
                generations: vec![first.clone()],
            },
            ChatCheckpointEnvelope {
                version: CHAT_CHECKPOINT_ENVELOPE_VERSION,
                generations: Vec::new(),
            },
            ChatCheckpointEnvelope {
                version: CHAT_CHECKPOINT_ENVELOPE_VERSION,
                generations: vec![first.clone(), first.clone()],
            },
        ] {
            let bytes = bson::serialize_to_vec(&malformed).unwrap();
            assert!(matches!(
                ChatCheckpointEnvelope::decode(&bytes),
                Err(StoreError::Malformed(_))
            ));
        }

        let mut tampered = first;
        tampered.payload.bytes[0] ^= 0xff;
        let bytes = bson::serialize_to_vec(&ChatCheckpointEnvelope {
            version: CHAT_CHECKPOINT_ENVELOPE_VERSION,
            generations: vec![tampered],
        })
        .unwrap();
        assert!(matches!(
            ChatCheckpointEnvelope::decode(&bytes),
            Err(StoreError::Malformed(_))
        ));
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

    #[tokio::test]
    async fn chat_staging_retains_the_committed_generation_until_pruned() {
        let (base, fixture, server) = storage_fixture().await;
        let store = ChronoStorageContextStore::new(
            base,
            "bkt",
            Arc::new(crate::store::token::StaticToken("tok".into())),
        );
        let session = SessionId("sesn_checkpoint".into());
        let first_bytes = b"conversation generation one".to_vec();
        let second_bytes = b"conversation generation two".to_vec();

        let first = store
            .stage_conversation(&session, None, first_bytes.clone())
            .await
            .expect("the first generation stages");
        let second = store
            .stage_conversation(&session, Some(&first.sha256), second_bytes.clone())
            .await
            .expect("the replacement stages beside the committed generation");
        assert_eq!(first.object_reference, second.object_reference);
        assert_eq!(
            store
                .read_conversation(&session, &first.object_reference, &first.sha256)
                .await
                .unwrap(),
            Some(first_bytes),
            "a crash before host CAS leaves the old digest readable"
        );
        assert_eq!(
            store
                .read_conversation(&session, &second.object_reference, &second.sha256)
                .await
                .unwrap(),
            Some(second_bytes)
        );
        let stored = fixture
            .objects
            .lock()
            .await
            .get(&first.object_reference)
            .cloned()
            .unwrap();
        assert_eq!(stored.content_type, CHAT_CHECKPOINT_CONTENT_TYPE);
        assert_eq!(
            ChatCheckpointEnvelope::decode(&stored.bytes)
                .unwrap()
                .generations
                .len(),
            2
        );

        store
            .prune_conversation(&session, &second.sha256)
            .await
            .expect("post-CAS pruning succeeds");
        assert_eq!(
            store
                .read_conversation(&session, &first.object_reference, &first.sha256)
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            store
                .read_conversation(&session, &second.object_reference, &second.sha256)
                .await
                .unwrap(),
            Some(b"conversation generation two".to_vec())
        );
        server.abort();
    }

    #[tokio::test]
    async fn chat_staging_requires_the_exact_retained_generation_and_reference() {
        let (base, fixture, server) = storage_fixture().await;
        let store = ChronoStorageContextStore::new(
            base,
            "bkt",
            Arc::new(crate::store::token::StaticToken("tok".into())),
        );
        let session = SessionId("sesn_checkpoint".into());
        let host = ChatHostId("chho_checkpoint".into());
        let first = store
            .stage_workspace(&host, None, b"workspace one".to_vec())
            .await
            .unwrap();
        assert_eq!(
            first.object_reference,
            "chat-hosts/chho_checkpoint/checkpoint-v1"
        );
        let before = fixture
            .objects
            .lock()
            .await
            .get(&first.object_reference)
            .unwrap()
            .bytes
            .clone();
        let missing = "f".repeat(64);
        assert!(matches!(
            store
                .stage_workspace(&host, Some(&missing), b"workspace two".to_vec())
                .await,
            Err(StoreError::Unavailable(_))
        ));
        assert_eq!(
            fixture
                .objects
                .lock()
                .await
                .get(&first.object_reference)
                .unwrap()
                .bytes,
            before,
            "a missing old generation cannot overwrite the valid object"
        );

        assert!(matches!(
            store
                .read_workspace(&host, "chat-hosts/chho_other/checkpoint-v1", &first.sha256,)
                .await,
            Err(StoreError::Malformed(_))
        ));
        assert!(matches!(
            store
                .read_conversation(
                    &session,
                    "sessions/sesn_other/handover.snapshot",
                    &first.sha256,
                )
                .await,
            Err(StoreError::Malformed(_))
        ));
        server.abort();
    }

    #[tokio::test]
    async fn chat_reads_fail_closed_when_the_stable_envelope_is_tampered() {
        let (base, fixture, server) = storage_fixture().await;
        let store = ChronoStorageContextStore::new(
            base,
            "bkt",
            Arc::new(crate::store::token::StaticToken("tok".into())),
        );
        let session = SessionId("sesn_tampered".into());
        let receipt = store
            .stage_conversation(&session, None, b"sealed".to_vec())
            .await
            .unwrap();
        let key = receipt.object_reference.clone();
        let mut objects = fixture.objects.lock().await;
        let object = objects.get_mut(&key).unwrap();
        let mut envelope = ChatCheckpointEnvelope::decode(&object.bytes).unwrap();
        envelope.generations[0].payload.bytes.push(0);
        object.bytes = bson::serialize_to_vec(&envelope).unwrap();
        drop(objects);

        assert!(matches!(
            store
                .read_conversation(&session, &key, &receipt.sha256)
                .await,
            Err(StoreError::Malformed(_))
        ));
        server.abort();
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
