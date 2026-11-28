use layer2_core::sandbox::store::SandboxStoreError;
use mongodb::bson::{self, spec::BinarySubtype, Binary, Document};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    corrupt, digest, MAX_BYTES, MAX_ENTRIES, MAX_SEAL_BYTES, MAX_SEQUENCE, MIGRATION, READER,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Head {
    #[serde(rename = "_id")]
    pub agent_id: String,
    pub format_version: i64,
    pub incarnation: String,
    pub revision: i64,
    pub active_ordinal: i64,
    pub active_start_seq: i64,
    pub last_seq: i64,
    pub active_count: i64,
    pub active_bytes: i64,
    #[serde(deserialize_with = "super::required_nullable")]
    pub previous_seal: Option<Binary>,
    pub lifetime_count: i64,
    pub lifetime_bytes: i64,
    pub projection_format: i64,
    pub evidence_generation: String,
    pub projection_through: i64,
    pub migration_id: String,
    pub required_reader: String,
}

impl Head {
    pub fn read(document: Document) -> Result<Self, SandboxStoreError> {
        require_int64(
            &document,
            &[
                "format_version",
                "revision",
                "active_ordinal",
                "active_start_seq",
                "last_seq",
                "active_count",
                "active_bytes",
                "lifetime_count",
                "lifetime_bytes",
                "projection_format",
                "projection_through",
            ],
        )?;
        let head: Self = bson::deserialize_from_document(document).map_err(|_| corrupt())?;
        head.validate()?;
        Ok(head)
    }

    pub fn validate(&self) -> Result<(), SandboxStoreError> {
        if self.format_version != 1
            || self.agent_id.is_empty()
            || self.agent_id.len() > 512
            || self.incarnation.len() != 64
            || !self
                .incarnation
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || self.revision < 1
            || self.active_ordinal < 0
            || !(0..=MAX_SEQUENCE).contains(&self.last_seq)
            || self.active_start_seq < 1
            || self.active_ordinal > self.active_start_seq - 1
            || self
                .last_seq
                .checked_sub(self.active_start_seq)
                .and_then(|n| n.checked_add(1))
                != Some(self.active_count)
            || !(0..=MAX_ENTRIES).contains(&self.active_count)
            || !(0..=MAX_BYTES).contains(&self.active_bytes)
            || (self.active_count == 0) != (self.active_bytes == 0)
            || self.lifetime_count != self.last_seq
            || self.lifetime_bytes < self.active_bytes
            || self.projection_format != 1
            || self.evidence_generation.is_empty()
            || self.evidence_generation.len() > 160
            || self.projection_through != self.last_seq
            || self.migration_id != MIGRATION
            || self.required_reader != READER
            || !predecessor_valid(self.active_ordinal, &self.previous_seal)
            || (self.active_ordinal == 0
                && (self.active_start_seq != 1 || self.lifetime_bytes != self.active_bytes))
        {
            return Err(corrupt());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Seal {
    #[serde(rename = "_id")]
    pub id: String,
    pub format_version: i64,
    pub agent_id: String,
    pub incarnation: String,
    pub ordinal: i64,
    pub start_seq: i64,
    pub end_seq: i64,
    pub entry_count: i64,
    pub encoded_bytes: i64,
    #[serde(deserialize_with = "super::required_nullable")]
    pub previous_seal: Option<Binary>,
    pub prefix_count: i64,
    pub prefix_bytes: i64,
    pub leaf_digests: Binary,
    pub seal_digest: Binary,
    pub digest_version: String,
    pub sealing_operation: String,
    pub source_format: String,
    pub storage_kind: String,
}

impl Seal {
    pub fn identity(agent: &str, incarnation: &str, ordinal: i64) -> String {
        format!("{agent}:{incarnation}:{ordinal:020}")
    }

    pub fn read(document: Document) -> Result<Self, SandboxStoreError> {
        require_int64(
            &document,
            &[
                "format_version",
                "ordinal",
                "start_seq",
                "end_seq",
                "entry_count",
                "encoded_bytes",
                "prefix_count",
                "prefix_bytes",
            ],
        )?;
        if bson::serialize_to_vec(&document)
            .map_err(|_| corrupt())?
            .len()
            >= MAX_SEAL_BYTES
        {
            return Err(corrupt());
        }
        let seal: Self = bson::deserialize_from_document(document).map_err(|_| corrupt())?;
        seal.validate()?;
        Ok(seal)
    }

    pub fn validate(&self) -> Result<(), SandboxStoreError> {
        if self.format_version != 1
            || self.agent_id.is_empty()
            || self.agent_id.len() > 512
            || self.incarnation.len() != 64
            || !self
                .incarnation
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || self.ordinal < 0
            || self.start_seq < 1
            || self.ordinal > self.start_seq - 1
            || self.end_seq > MAX_SEQUENCE
            || self
                .end_seq
                .checked_sub(self.start_seq)
                .and_then(|n| n.checked_add(1))
                != Some(self.entry_count)
            || !(1..=MAX_ENTRIES).contains(&self.entry_count)
            || !(1..=MAX_BYTES).contains(&self.encoded_bytes)
            || self.prefix_count != self.end_seq
            || self.prefix_bytes < self.encoded_bytes
            || self.leaf_digests.subtype != BinarySubtype::Generic
            || i64::try_from(self.leaf_digests.bytes.len()).ok() != self.entry_count.checked_mul(32)
            || !predecessor_valid(self.ordinal, &self.previous_seal)
            || (self.ordinal == 0
                && (self.start_seq != 1 || self.prefix_bytes != self.encoded_bytes))
            || self.id != Self::identity(&self.agent_id, &self.incarnation, self.ordinal)
            || self.digest_version != digest::VERSION
            || self.storage_kind != "mongo"
            || !matches!(self.source_format.as_str(), "legacy_v1" | "segmented_v1")
            || self.sealing_operation.is_empty()
            || self.sealing_operation.len() > 512
            || self.seal_digest != self.digest()
        {
            return Err(corrupt());
        }
        Ok(())
    }

    pub fn digest(&self) -> Binary {
        let mut hash = Sha256::new();
        hash.update(b"cma.agent-log.seal.sha256_raw_bson_v1\0");
        for field in [&self.id, &self.agent_id, &self.incarnation] {
            digest::field(&mut hash, field.as_bytes());
        }
        for n in [
            self.format_version,
            self.ordinal,
            self.start_seq,
            self.end_seq,
            self.entry_count,
            self.encoded_bytes,
            self.prefix_count,
            self.prefix_bytes,
        ] {
            hash.update(n.to_be_bytes());
        }
        digest::field(
            &mut hash,
            self.previous_seal.as_ref().map_or(&[], |v| &v.bytes),
        );
        digest::field(&mut hash, &self.leaf_digests.bytes);
        for field in [
            &self.digest_version,
            &self.sealing_operation,
            &self.source_format,
            &self.storage_kind,
        ] {
            digest::field(&mut hash, field.as_bytes());
        }
        digest::binary(hash.finalize().to_vec())
    }

    pub fn follows(&self, previous: &Self) -> Result<(), SandboxStoreError> {
        if self.agent_id != previous.agent_id
            || self.incarnation != previous.incarnation
            || previous.ordinal.checked_add(1) != Some(self.ordinal)
            || previous.end_seq.checked_add(1) != Some(self.start_seq)
            || self.previous_seal.as_ref() != Some(&previous.seal_digest)
            || previous.prefix_bytes.checked_add(self.encoded_bytes) != Some(self.prefix_bytes)
        {
            return Err(corrupt());
        }
        Ok(())
    }

    pub fn precedes(&self, head: &Head) -> Result<(), SandboxStoreError> {
        if self.agent_id != head.agent_id
            || self.incarnation != head.incarnation
            || self.ordinal.checked_add(1) != Some(head.active_ordinal)
            || self.end_seq.checked_add(1) != Some(head.active_start_seq)
            || head.previous_seal.as_ref() != Some(&self.seal_digest)
            || self.prefix_bytes.checked_add(head.active_bytes) != Some(head.lifetime_bytes)
        {
            return Err(corrupt());
        }
        Ok(())
    }
}

fn require_int64(document: &Document, fields: &[&str]) -> Result<(), SandboxStoreError> {
    if fields.iter().any(|field| document.get_i64(field).is_err()) {
        return Err(corrupt());
    }
    Ok(())
}

fn predecessor_valid(ordinal: i64, digest: &Option<Binary>) -> bool {
    match (ordinal, digest) {
        (0, None) => true,
        (n, Some(value)) if n > 0 => digest::valid_digest(value),
        _ => false,
    }
}
