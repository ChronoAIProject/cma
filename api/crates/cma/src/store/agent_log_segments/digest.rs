use mongodb::bson::{spec::BinarySubtype, Binary};
use sha2::{Digest, Sha256};

use super::corrupt;
use layer2_core::sandbox::store::SandboxStoreError;

pub(in crate::store) const VERSION: &str = "sha256_raw_bson_v1";

pub(in crate::store) fn binary(bytes: impl Into<Vec<u8>>) -> Binary {
    Binary {
        subtype: BinarySubtype::Generic,
        bytes: bytes.into(),
    }
}

pub(in crate::store) fn valid_digest(value: &Binary) -> bool {
    value.subtype == BinarySubtype::Generic && value.bytes.len() == 32
}

/// All lengths are unsigned 64-bit big-endian byte lengths. Numeric manifest
/// fields are nonnegative signed BSON integers, hashed as big-endian u64.
pub(super) fn field(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u64).to_be_bytes());
    hash.update(bytes);
}

pub(in crate::store) fn leaf(agent: &str, incarnation: &str, seq: i64, raw: &[u8]) -> Binary {
    let mut hash = Sha256::new();
    hash.update(b"cma.agent-log.leaf.sha256_raw_bson_v1\0");
    field(&mut hash, agent.as_bytes());
    field(&mut hash, incarnation.as_bytes());
    hash.update(seq.to_be_bytes());
    field(&mut hash, raw);
    binary(hash.finalize().to_vec())
}

pub(in crate::store) fn incarnation(agent: &layer2_core::sandbox::Agent) -> String {
    incarnation_fields(agent.id.as_str(), &agent.creation_key, &agent.created_at)
}

pub(in crate::store) fn incarnation_fields(
    id: &str,
    creation_key: &str,
    created_at: &str,
) -> String {
    let mut hash = Sha256::new();
    hash.update(b"cma.agent-log.incarnation.v1\0");
    for value in [id, creation_key, created_at] {
        field(&mut hash, value.as_bytes());
    }
    format!("{:x}", hash.finalize())
}

pub(super) fn check_leaf(
    leaves: &Binary,
    offset: i64,
    actual: &Binary,
) -> Result<(), SandboxStoreError> {
    let start = usize::try_from(offset)
        .ok()
        .and_then(|n| n.checked_mul(32))
        .ok_or_else(corrupt)?;
    if leaves
        .bytes
        .get(start..start.checked_add(32).ok_or_else(corrupt)?)
        != Some(actual.bytes.as_slice())
    {
        return Err(corrupt());
    }
    Ok(())
}
