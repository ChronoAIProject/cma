//! Authenticated, bounded relay frames using the gateway's existing AEAD keys.
//! Each chunk is bound to the complete plaintext digest so chunks from another
//! request cannot be spliced into an otherwise valid envelope.
use super::super::{storage, ExecutionError};
use cmaeg_core::seal::{open_interaction, seal_interaction, MasterKeyRing};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

pub(in crate::turns::routed) const BODY_LIMIT: usize = 384 * 1024;
const PLAINTEXT_LIMIT: usize = 192 * 1024;
pub(in crate::turns::routed) const STREAM_BODY_LIMIT: usize = 768 * 1024;
const CHUNK: usize = 32 * 1024;
const MAX_LIFETIME: i64 = 25;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::turns::routed) struct Envelope {
    pub id: String,
    manifest: String,
    chunks: Vec<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u8,
    context: String,
    expires_at: i64,
    bytes: usize,
    sha256: String,
    chunks: usize,
}
#[derive(Clone, Copy)]
pub(in crate::turns::routed) enum Direction {
    Request,
    Response,
    StreamRequest,
    StreamResponse,
}
impl Direction {
    fn plaintext_limit(self) -> usize {
        match self {
            Self::StreamRequest | Self::StreamResponse => 320 * 1024,
            _ => PLAINTEXT_LIMIT,
        }
    }
    fn domain(self) -> &'static str {
        match self {
            Self::Request => "cmaeg-replica-tool-request-v1",
            Self::Response => "cmaeg-replica-tool-response-v1",
            Self::StreamRequest => "cmaeg-replica-stream-request-v1",
            Self::StreamResponse => "cmaeg-replica-stream-response-v1",
        }
    }
}

pub(in crate::turns::routed) fn seal(
    keys: &MasterKeyRing,
    direction: Direction,
    id: &str,
    context: &str,
    expires_at: i64,
    now: i64,
    value: &impl Serialize,
) -> Result<Envelope, ExecutionError> {
    validate_id(id)?;
    validate_expiry(expires_at, now)?;
    if context.len() > 128 {
        return Err(ExecutionError::Invalid);
    }
    let plaintext = storage::bytes(value)?;
    if plaintext.is_empty() || plaintext.len() > direction.plaintext_limit() {
        return Err(ExecutionError::Invalid);
    }
    let manifest = Manifest {
        version: 1,
        context: context.into(),
        expires_at,
        bytes: plaintext.len(),
        sha256: storage::hash(&plaintext),
        chunks: plaintext.len().div_ceil(CHUNK),
    };
    let domain = format!("{}:{id}", direction.domain());
    let chunks = plaintext
        .chunks(CHUNK)
        .enumerate()
        .map(|(index, bytes)| {
            seal_text(
                keys,
                &format!("{domain}:{}:{index}", manifest.sha256),
                bytes,
            )
        })
        .collect::<Result<_, _>>()?;
    Ok(Envelope {
        id: id.into(),
        manifest: seal_text(keys, &domain, &storage::bytes(&manifest)?)?,
        chunks,
    })
}

pub(in crate::turns::routed) fn open<T: DeserializeOwned>(
    keys: &MasterKeyRing,
    direction: Direction,
    context: &str,
    now: i64,
    envelope: &Envelope,
) -> Result<T, ExecutionError> {
    validate_id(&envelope.id)?;
    if envelope.manifest.len() > 2048
        || envelope.chunks.len() > direction.plaintext_limit().div_ceil(CHUNK)
        || envelope.chunks.iter().any(|chunk| chunk.len() > 65536)
    {
        return Err(ExecutionError::Invalid);
    }
    let domain = format!("{}:{}", direction.domain(), envelope.id);
    let manifest: Manifest = serde_json::from_slice(
        &open_interaction(keys, &domain, envelope.manifest.as_bytes())
            .map_err(|_| ExecutionError::AuthorityChanged)?
            .payload,
    )
    .map_err(|_| ExecutionError::Invalid)?;
    validate_expiry(manifest.expires_at, now)?;
    if manifest.version != 1
        || manifest.context != context
        || manifest.bytes == 0
        || manifest.bytes > direction.plaintext_limit()
        || manifest.chunks != manifest.bytes.div_ceil(CHUNK)
        || envelope.chunks.len() != manifest.chunks
        || manifest.sha256.len() != 64
    {
        return Err(ExecutionError::Invalid);
    }
    let mut plaintext = Vec::with_capacity(manifest.bytes);
    for (index, chunk) in envelope.chunks.iter().enumerate() {
        let bytes = open_interaction(
            keys,
            &format!("{domain}:{}:{index}", manifest.sha256),
            chunk.as_bytes(),
        )
        .map_err(|_| ExecutionError::AuthorityChanged)?
        .payload;
        let expected = (manifest.bytes - index * CHUNK).min(CHUNK);
        if bytes.len() != expected {
            return Err(ExecutionError::Invalid);
        }
        plaintext.extend_from_slice(&bytes);
    }
    if storage::hash(&plaintext) != manifest.sha256 {
        return Err(ExecutionError::Invalid);
    }
    serde_json::from_slice(&plaintext).map_err(|_| ExecutionError::Invalid)
}
fn seal_text(keys: &MasterKeyRing, domain: &str, bytes: &[u8]) -> Result<String, ExecutionError> {
    String::from_utf8(
        seal_interaction(keys, domain, bytes).map_err(|_| ExecutionError::Unavailable)?,
    )
    .map_err(|_| ExecutionError::Unavailable)
}
fn validate_id(id: &str) -> Result<(), ExecutionError> {
    let parsed = uuid::Uuid::parse_str(id).map_err(|_| ExecutionError::Invalid)?;
    if parsed.get_version_num() != 4 || parsed.to_string() != id {
        return Err(ExecutionError::Invalid);
    }
    Ok(())
}
fn validate_expiry(expires_at: i64, now: i64) -> Result<(), ExecutionError> {
    if expires_at <= now || expires_at > now.saturating_add(MAX_LIFETIME) {
        return Err(ExecutionError::Expired);
    }
    Ok(())
}
