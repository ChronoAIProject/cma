//! Private authenticated ledger bindings. A valid binding is not maintenance authority.

use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};

pub const LEDGER_BINDING_FORMAT: &str = "runtime-ledger-binding-v1";
pub const MAX_LEDGER_BINDING_BYTES: usize = 8 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LedgerSourceFormatV1 {
    V1,
    V2,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerBindingV1 {
    #[serde(deserialize_with = "format")]
    pub format: String,
    #[serde(deserialize_with = "identity")]
    pub sandbox_id: String,
    #[serde(deserialize_with = "home")]
    pub home_identity: String,
    #[serde(deserialize_with = "positive")]
    pub generation: u64,
    #[serde(deserialize_with = "boot")]
    pub boot_id: String,
    #[serde(deserialize_with = "digest")]
    pub roots_sha256: String,
    pub source_format: LedgerSourceFormatV1,
    #[serde(deserialize_with = "archive")]
    pub archive_id: String,
    #[serde(deserialize_with = "identity")]
    pub maintenance_id: String,
    #[serde(deserialize_with = "positive")]
    pub maintenance_epoch: u64,
    #[serde(deserialize_with = "positive")]
    pub sandbox_revision: u64,
    #[serde(deserialize_with = "digest")]
    pub candidate_sha256: String,
    #[serde(deserialize_with = "digest")]
    pub recovery_sha256: String,
    #[serde(deserialize_with = "digest")]
    pub exclusion_receipt_sha256: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum LedgerBindingRefusal {
    #[error("ledger binding exceeds the supported byte bound")]
    Oversized,
    #[error("ledger binding schema or evidence is invalid")]
    InvalidEvidence,
    #[error("ledger binding differs from the accepted binding")]
    BindingChanged,
}

impl LedgerBindingV1 {
    /// Revalidate in-process values too: wire fields are public for the frozen contract.
    pub fn validate(&self) -> Result<(), LedgerBindingRefusal> {
        if self.format != LEDGER_BINDING_FORMAT
            || !valid_identity(&self.sandbox_id)
            || !crate::valid_home_identity(&self.home_identity)
            || self.generation == 0
            || !lower_hex(&self.boot_id, 32)
            || !valid_sha256(&self.roots_sha256)
            || !valid_archive(&self.archive_id)
            || !valid_identity(&self.maintenance_id)
            || self.maintenance_epoch == 0
            || self.sandbox_revision == 0
            || !valid_sha256(&self.candidate_sha256)
            || !valid_sha256(&self.recovery_sha256)
            || !valid_sha256(&self.exclusion_receipt_sha256)
        {
            return Err(LedgerBindingRefusal::InvalidEvidence);
        }
        Ok(())
    }

    pub fn validate_for(&self, accepted: &Self) -> Result<(), LedgerBindingRefusal> {
        self.validate()?;
        accepted.validate()?;
        if self != accepted {
            return Err(LedgerBindingRefusal::BindingChanged);
        }
        Ok(())
    }

    /// Structured identity only. This is deliberately NOT the artifact byte hash.
    pub fn binding_sha256(&self) -> Result<String, LedgerBindingRefusal> {
        self.validate()?;
        Ok(self.identity_hash())
    }

    fn identity_hash(&self) -> String {
        let generation = self.generation.to_be_bytes();
        let epoch = self.maintenance_epoch.to_be_bytes();
        let revision = self.sandbox_revision.to_be_bytes();
        let source: &[u8] = match self.source_format {
            LedgerSourceFormatV1::V1 => b"v1",
            LedgerSourceFormatV1::V2 => b"v2",
        };
        let parts: [&[u8]; 15] = [
            self.format.as_bytes(),
            self.sandbox_id.as_bytes(),
            self.home_identity.as_bytes(),
            &generation,
            self.boot_id.as_bytes(),
            self.roots_sha256.as_bytes(),
            source,
            self.archive_id.as_bytes(),
            self.maintenance_id.as_bytes(),
            &epoch,
            &revision,
            self.candidate_sha256.as_bytes(),
            self.recovery_sha256.as_bytes(),
            self.exclusion_receipt_sha256.as_bytes(),
            // Pin the structured encoding independently of the JSON wire format.
            b"length-prefixed-be-v1",
        ];
        let mut hash = Sha256::new();
        hash.update(b"chrono.runtime.ledger.binding.v1\0");
        for part in parts {
            hash.update((part.len() as u64).to_be_bytes());
            hash.update(part);
        }
        format!("{:x}", hash.finalize())
    }
}

impl std::fmt::Debug for LedgerBindingV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LedgerBindingV1")
            .field("binding_sha256", &self.identity_hash())
            .finish()
    }
}

/// Retains the original artifact. Persistence must store these bytes before effects.
/// Parsing/holding this value does not itself persist or authenticate anything.
#[derive(Clone)]
pub struct LedgerBindingArtifactV1 {
    bytes: Vec<u8>,
    bytes_sha256: String,
    binding: LedgerBindingV1,
}

impl LedgerBindingArtifactV1 {
    pub fn parse(bytes: &[u8]) -> Result<Self, LedgerBindingRefusal> {
        if bytes.len() > MAX_LEDGER_BINDING_BYTES {
            return Err(LedgerBindingRefusal::Oversized);
        }
        let binding: LedgerBindingV1 =
            serde_json::from_slice(bytes).map_err(|_| LedgerBindingRefusal::InvalidEvidence)?;
        binding.validate()?;
        Ok(Self {
            bytes: bytes.to_vec(),
            bytes_sha256: format!("{:x}", Sha256::digest(bytes)),
            binding,
        })
    }

    pub fn original_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn bytes_sha256(&self) -> &str {
        &self.bytes_sha256
    }

    pub fn binding(&self) -> &LedgerBindingV1 {
        &self.binding
    }
}

impl std::fmt::Debug for LedgerBindingArtifactV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LedgerBindingArtifactV1")
            .field("bytes", &self.bytes.len())
            .field("bytes_sha256", &self.bytes_sha256)
            .finish()
    }
}

pub fn valid_sha256(value: &str) -> bool {
    lower_hex(value, 64)
}

fn lower_hex(value: &str, size: usize) -> bool {
    value.len() == size
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

fn valid_archive(value: &str) -> bool {
    uuid::Uuid::parse_str(value)
        .map(|id| !id.is_nil() && id.to_string() == value)
        .unwrap_or(false)
}

fn checked<'de, D: Deserializer<'de>>(
    deserializer: D,
    valid: impl FnOnce(&str) -> bool,
) -> Result<String, D::Error> {
    let value = String::deserialize(deserializer)?;
    if !valid(&value) {
        return Err(serde::de::Error::custom("invalid ledger binding field"));
    }
    Ok(value)
}

fn format<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    checked(d, |value| value == LEDGER_BINDING_FORMAT)
}
fn identity<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    checked(d, valid_identity)
}
fn home<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    checked(d, crate::valid_home_identity)
}
fn boot<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    checked(d, |value| lower_hex(value, 32))
}
fn digest<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    checked(d, valid_sha256)
}
fn archive<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    checked(d, valid_archive)
}
fn positive<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    let value = u64::deserialize(d)?;
    if value == 0 {
        return Err(serde::de::Error::custom("ledger revision must be positive"));
    }
    Ok(value)
}

#[cfg(test)]
#[path = "runtime_ledger_maintenance_tests.rs"]
mod tests;
