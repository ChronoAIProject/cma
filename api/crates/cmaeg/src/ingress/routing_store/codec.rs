use super::types::*;
use cmaeg_core::{profile::canonical_json, seal::MAX_BLOB_BYTES};
use serde::{de::DeserializeOwned, Serialize};
use sha2::{Digest, Sha256};

pub(super) fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub(super) fn is_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
}
pub(super) fn canonical(value: &impl Serialize) -> Result<Vec<u8>, AdmissionError> {
    let value = serde_json::to_value(value).map_err(|_| AdmissionError::Invalid)?;
    Ok(canonical_json(value).into_bytes())
}
pub(super) fn identifier(domain: &str, value: &impl Serialize) -> String {
    let mut sha = Sha256::new();
    sha.update(domain.as_bytes());
    sha.update([0]);
    sha.update(canonical(value).expect("routing identifiers serialize"));
    let mut bytes: [u8; 16] = sha.finalize()[..16].try_into().unwrap();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(bytes).to_string()
}

pub(super) fn request_hash(
    activity: &cmaeg_core::activity::Activity,
    raw: &[u8],
) -> Result<String, AdmissionError> {
    let mut activity = serde_json::to_value(activity).map_err(|_| AdmissionError::Invalid)?;
    // Routing and fallback ingress wall-clock time are not logical request data.
    // Explicit producer time remains inside the preserved raw event/CloudEvent.
    let object = activity.as_object_mut().ok_or(AdmissionError::Invalid)?;
    object.remove("routing");
    object.remove("occurred_at");
    let source = match serde_json::from_slice::<serde_json::Value>(raw) {
        Ok(value) => canonical(&value)?,
        Err(_) => raw.to_vec(),
    };
    Ok(hash(&canonical(&(activity, hash(&source)))?))
}

pub(super) fn encode(value: &impl Serialize) -> Result<Vec<u8>, AdmissionError> {
    let bytes = canonical(value)?;
    if bytes.len() > MAX_BLOB_BYTES {
        return Err(AdmissionError::TooLarge);
    }
    Ok(bytes)
}
pub(super) fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, AdmissionError> {
    serde_json::from_slice(bytes).map_err(|_| AdmissionError::Corrupt)
}

pub(super) fn chunk_bytes(
    manifest: &Manifest,
    index: usize,
    bytes: &[u8],
) -> Result<Vec<u8>, AdmissionError> {
    encode(&Chunk {
        kind: CHUNK_KIND.into(),
        root: manifest.id.clone(),
        request_hash: manifest.request_hash.clone(),
        payload_hash: manifest.payload_hash.clone(),
        index,
        count: manifest.chunk_hashes.len(),
        retained_until: manifest.retained_until,
        bytes: bytes.to_vec(),
    })
}
pub(super) fn open_chunk(
    manifest: &Manifest,
    index: usize,
    bytes: &[u8],
) -> Result<Vec<u8>, AdmissionError> {
    let chunk: Chunk = decode(bytes)?;
    let expected_len = (manifest.payload_len - index * CHUNK_BYTES).min(CHUNK_BYTES);
    if chunk.kind != CHUNK_KIND
        || chunk.root != manifest.id
        || chunk.request_hash != manifest.request_hash
        || chunk.payload_hash != manifest.payload_hash
        || chunk.index != index
        || chunk.count != manifest.chunk_hashes.len()
        || chunk.retained_until != manifest.retained_until
        || chunk.bytes.len() != expected_len
        || hash(&chunk.bytes) != manifest.chunk_hashes[index]
    {
        return Err(AdmissionError::Corrupt);
    }
    Ok(chunk.bytes)
}

pub(super) mod base64_bytes {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use serde::{de::Error, Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(bytes))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        STANDARD
            .decode(String::deserialize(deserializer)?)
            .map_err(D::Error::custom)
    }
}
