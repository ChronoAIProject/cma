//! Wire identity and minimal evidence for a single Sandbox creation.

use layer2_core::{operation::valid_idempotency_key, SubjectId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use utoipa::ToSchema;

pub(super) mod routes;
pub use routes::{problem, router, AuthorizedLookup, Backend, Begin, Submission};

pub const LOOKUP_PATH: &str = "/api/v1/sandbox-creation-outcomes/lookup";
const KEY_PREFIX: &str = "cma1.";
pub const MAX_LOOKUP_BYTES: usize = 2048;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Sandbox,
    AgentBootstrap,
}

impl Operation {
    pub const fn path(self) -> &'static str {
        match self {
            Self::Sandbox => "/api/v1/sandboxes",
            Self::AgentBootstrap => "/api/v1/agents",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, ToSchema)]
pub enum Algorithm {
    #[serde(rename = "nyxid-wire-v1")]
    NyxidWire,
    #[serde(rename = "sha256-wire-v1")]
    Sha256Wire,
}

impl Algorithm {
    const fn tag(self) -> &'static str {
        match self {
            Self::NyxidWire => "nyxid-wire-v1",
            Self::Sha256Wire => "sha256-wire-v1",
        }
    }
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RequestFingerprint {
    pub algorithm: Algorithm,
    pub digest: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LookupRequest {
    pub version: u8,
    pub deployment: String,
    pub method: String,
    pub path: String,
    pub idempotency_key: String,
    pub fingerprint: RequestFingerprint,
    pub claim_id: Option<String>,
    pub operation_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityError {
    Invalid,
    Mismatch,
}

pub struct KeyPeriod<'a> {
    pub epoch: &'a str,
    pub day: i64,
}

pub struct Identity<'a> {
    pub owner: &'a SubjectId,
    pub deployment: &'a str,
    pub operation: Operation,
    pub key: &'a str,
    pub algorithm: Algorithm,
    pub wire_digest: &'a str,
}

impl LookupRequest {
    pub fn identity<'a>(
        &'a self,
        owner: &'a SubjectId,
        deployment: &str,
    ) -> Result<Identity<'a>, IdentityError> {
        let operation = match self.path.as_str() {
            "/api/v1/sandboxes" if self.operation_id.is_none() => Operation::Sandbox,
            "/api/v1/agents" if self.claim_id.is_none() => Operation::AgentBootstrap,
            _ => return Err(IdentityError::Invalid),
        };
        if self.version != 1
            || self.method != "POST"
            || !lower_hex(&self.deployment, 64)
            || !lower_hex(&self.fingerprint.digest, 64)
            || !valid_idempotency_key(self.idempotency_key.as_bytes())
            || [&self.claim_id, &self.operation_id]
                .into_iter()
                .flatten()
                .any(|id| !lower_hex(id, 32))
        {
            return Err(IdentityError::Invalid);
        }
        if self.deployment != deployment {
            return Err(IdentityError::Mismatch);
        }
        Ok(Identity {
            owner,
            deployment: &self.deployment,
            operation,
            key: &self.idempotency_key,
            algorithm: self.fingerprint.algorithm,
            wire_digest: &self.fingerprint.digest,
        })
    }
}

struct CoveredKey<'a> {
    epoch: &'a str,
    day: &'a str,
    day_number: u32,
    nonce: &'a str,
    binding: &'a str,
}

impl<'a> CoveredKey<'a> {
    fn parse(key: &'a str) -> Option<Self> {
        let mut parts = key.strip_prefix(KEY_PREFIX)?.split('.');
        let epoch = parts.next()?;
        let day = parts.next()?;
        let nonce = parts.next()?;
        let binding = parts.next()?;
        let day_number = day
            .parse::<u32>()
            .ok()
            .filter(|value| value.to_string() == day)?;
        if parts.next().is_some()
            || !lower_hex(epoch, 32)
            || !lower_hex(nonce, 32)
            || !lower_hex(binding, 64)
        {
            return None;
        }
        Some(Self {
            epoch,
            day,
            day_number,
            nonce,
            binding,
        })
    }
}

/// A covered key has the v1 wire shape. Its binding is checked separately.
pub fn is_covered_key(key: &str) -> bool {
    CoveredKey::parse(key).is_some()
}

impl Identity<'_> {
    pub fn period(&self) -> Result<Option<KeyPeriod<'_>>, IdentityError> {
        let Some(CoveredKey {
            epoch,
            day,
            day_number,
            nonce,
            binding,
        }) = CoveredKey::parse(self.key)
        else {
            return Ok(None);
        };
        let expected = hash_fields(&[
            "cma-creation-key-v1",
            epoch,
            day,
            nonce,
            self.owner.as_str(),
            self.deployment,
            "POST",
            self.operation.path(),
            self.wire_digest,
        ]);
        if self.algorithm != Algorithm::NyxidWire || binding != expected {
            return Err(IdentityError::Mismatch);
        }
        Ok(Some(KeyPeriod {
            epoch,
            day: i64::from(day_number),
        }))
    }

    pub fn digest(&self) -> String {
        hash_fields(&[
            "cma-creation-identity-v1",
            self.owner.as_str(),
            self.deployment,
            "POST",
            self.operation.path(),
            self.key,
            self.algorithm.tag(),
            self.wire_digest,
        ])
    }
}

pub fn lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub fn hash_fields(fields: &[&str]) -> String {
    let mut hash = Sha256::new();
    for field in fields {
        hash.update(
            u32::try_from(field.len())
                .expect("bounded identity field")
                .to_be_bytes(),
        );
        hash.update(field.as_bytes());
    }
    format!("{:x}", hash.finalize())
}

pub fn wire_digest(bytes: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(b"nyxid:cma-sandbox-creation:wire-v1\0");
    hash.update(bytes);
    format!("{:x}", hash.finalize())
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RefusalCode {
    SandboxLimitReached,
    TrustConfirmationRequired,
    CreationFailed,
    CreationCancelled,
    CreationExpired,
}

/// The local semantic validation result to bind to the creation proof.
#[derive(Clone, Copy, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ValidationRefusal {
    TaskTemplate {
        code: &'static str,
        field: Option<&'static str>,
    },
    DesktopInitialMessageRequired,
}

impl RefusalCode {
    pub const fn code(self) -> &'static str {
        match self {
            Self::SandboxLimitReached => "sandbox_limit_reached",
            Self::TrustConfirmationRequired => "trust_confirmation_required",
            Self::CreationFailed => "creation_failed",
            Self::CreationCancelled => "creation_cancelled",
            Self::CreationExpired => "creation_expired",
        }
    }
}

#[derive(Debug, Serialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum UnknownReason {
    NoDurableEvidence,
    LegacyFingerprintUnavailable,
    GenerationAmbiguous,
    RecoveryRequired,
    CoverageUnavailable,
}

#[derive(Debug, Serialize, PartialEq, Eq, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Outcome {
    Created {
        sandbox_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        operation_id: Option<String>,
        proof_id: String,
    },
    Refused {
        code: RefusalCode,
        #[serde(skip_serializing_if = "Option::is_none")]
        operation_id: Option<String>,
        proof_id: String,
    },
    NeverReceived {
        proof_id: String,
    },
    InProgress {
        #[serde(skip_serializing_if = "Option::is_none")]
        operation_id: Option<String>,
    },
    Unknown {
        reason: UnknownReason,
    },
}

#[derive(Serialize, ToSchema)]
pub struct LookupResponse {
    pub version: u8,
    pub deployment: String,
    pub operation: Operation,
    pub identity_digest: String,
    pub result: Outcome,
}

impl Outcome {
    pub fn created(
        digest: &str,
        generation: &str,
        sandbox_id: String,
        operation_id: Option<String>,
    ) -> Self {
        let proof_id = hash_fields(&[
            "cma-creation-proof-v1",
            digest,
            generation,
            "created",
            &sandbox_id,
            operation_id.as_deref().unwrap_or(""),
        ]);
        Self::Created {
            sandbox_id,
            operation_id,
            proof_id,
        }
    }

    pub fn refused(
        digest: &str,
        generation: &str,
        code: RefusalCode,
        operation_id: Option<String>,
    ) -> Self {
        let proof_id = hash_fields(&[
            "cma-creation-proof-v1",
            digest,
            generation,
            "refused",
            code.code(),
            operation_id.as_deref().unwrap_or(""),
        ]);
        Self::Refused {
            code,
            operation_id,
            proof_id,
        }
    }

    pub fn never_received(digest: &str, period: &KeyPeriod<'_>) -> Self {
        let proof_id = hash_fields(&[
            "cma-creation-proof-v1",
            digest,
            period.epoch,
            &period.day.to_string(),
            "never_received",
        ]);
        Self::NeverReceived { proof_id }
    }
}

#[cfg(test)]
mod tests;
