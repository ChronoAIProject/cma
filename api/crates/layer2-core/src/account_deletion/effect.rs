use serde::{Deserialize, Serialize};

/// Fingerprint of durable evidence; possession does not prove its authority.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ErasureDigest(String);

impl TryFrom<String> for ErasureDigest {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len() != 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err("erasure evidence digest must contain 64 lowercase hexadecimal digits");
        }
        Ok(Self(value))
    }
}

impl From<ErasureDigest> for String {
    fn from(value: ErasureDigest) -> Self {
        value.0
    }
}

/// The sealed effect ledger and writer barrier that a later census must follow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErasureEffectCheckpoint {
    pub inventory_generation: u64,
    pub effects_digest: ErasureDigest,
    pub effects_completed_at: String,
    pub writer_drain_digest: ErasureDigest,
    pub writers_closed_at: String,
}
