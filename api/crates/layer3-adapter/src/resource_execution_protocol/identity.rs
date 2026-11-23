use std::fmt;

use serde::{de::Visitor, Deserialize, Deserializer, Serialize};

use super::{ProtocolError, ProtocolResult};

/// Opaque identifiers are bounded before the decoder copies them and never
/// appear in diagnostic formatting. They carry correlation, not authority.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct OpaqueId(String);

impl OpaqueId {
    pub fn new(value: &str) -> ProtocolResult<Self> {
        if value.trim().is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
            return Err(ProtocolError::InvalidIdentity);
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for OpaqueId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OpaqueId([redacted])")
    }
}

impl<'de> Deserialize<'de> for OpaqueId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct IdVisitor;
        impl Visitor<'_> for IdVisitor {
            type Value = OpaqueId;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a bounded opaque identifier")
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                OpaqueId::new(value).map_err(E::custom)
            }
        }
        deserializer.deserialize_str(IdVisitor)
    }
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct Digest(String);

impl Digest {
    pub fn new(value: &str) -> ProtocolResult<Self> {
        if value.len() != 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(ProtocolError::InvalidIdentity);
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Digest([redacted])")
    }
}

impl<'de> Deserialize<'de> for Digest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct DigestVisitor;
        impl Visitor<'_> for DigestVisitor {
            type Value = Digest;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a lowercase SHA-256 digest")
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Digest::new(value).map_err(E::custom)
            }
        }
        deserializer.deserialize_str(DigestVisitor)
    }
}

/// Generation zero is never authority. The upper bound matches Mongo's signed
/// integer representation, so a valid wire value can be persisted losslessly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct Generation(u64);

impl Generation {
    pub fn new(value: u64) -> ProtocolResult<Self> {
        if value == 0 || value > i64::MAX as u64 {
            return Err(ProtocolError::InvalidIdentity);
        }
        Ok(Self(value))
    }

    pub fn get(self) -> u64 {
        self.0
    }

    pub fn successor(self) -> ProtocolResult<Self> {
        Self::new(self.0.checked_add(1).ok_or(ProtocolError::Capacity)?)
    }
}

impl<'de> Deserialize<'de> for Generation {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(u64::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// All recipients and descendants of one originating share collide here.
/// Policy revision, capability, actor and manager epoch belong only in proofs.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantFamilyKey {
    pub origin: Digest,
    pub share_id: OpaqueId,
    pub generation: Generation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantKey {
    pub family: GrantFamilyKey,
    pub share_revision: Generation,
    pub target_ancestry: Digest,
    pub capability: ProtectedCapability,
    pub policy_revision: Generation,
    pub actor: Digest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtectedCapability {
    ReadDetails,
    ReadHistory,
    ReadArtifacts,
    ReadFiles,
    Prompt,
    ResolveApproval,
    OperateAgent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExecutionBasisV1 {
    Share {
        grant: GrantKey,
    },
    Independent {
        authority: IndependentAuthorityV1,
        owner: Digest,
        actor: Digest,
        target_ancestry: Digest,
        capability: ProtectedCapability,
    },
    IndependentOwner {
        owner: Digest,
        actor: Digest,
        target_ancestry: Digest,
        capability: ProtectedCapability,
    },
}

/// Separate provenance keeps operator, organization and binding authority from
/// being replayed as personal ownership. These values are proof identity only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum IndependentAuthorityV1 {
    GlobalOperator,
    OwningOrganization {
        issuer: Digest,
        organization: Digest,
        role: Digest,
    },
    ExternalBinding {
        binding: Digest,
        generation: Generation,
    },
}

impl ExecutionBasisV1 {
    pub fn capability(&self) -> ProtectedCapability {
        match self {
            Self::Share { grant } => grant.capability,
            Self::IndependentOwner { capability, .. } | Self::Independent { capability, .. } => {
                *capability
            }
        }
    }

    pub fn validate(&self) -> ProtocolResult<()> {
        if let Self::Share { grant } = self {
            if grant.share_revision > grant.policy_revision {
                return Err(ProtocolError::InvalidIdentity);
            }
        }
        Ok(())
    }

    pub fn family(&self) -> Option<&GrantFamilyKey> {
        match self {
            Self::Share { grant } => Some(&grant.family),
            Self::IndependentOwner { .. } | Self::Independent { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiverKind {
    Runtime,
    ReleaseProducer,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiverKey {
    pub kind: ReceiverKind,
    pub instance: OpaqueId,
    pub incarnation: Digest,
    pub boot: OpaqueId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionKey {
    pub command_id: OpaqueId,
    pub command_fingerprint: Digest,
    pub dispatch_sequence: Generation,
    pub prompt_id: OpaqueId,
    pub turn_id: OpaqueId,
    pub agent_id: OpaqueId,
    pub receiver: ReceiverKey,
    pub attempt: Generation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseKey {
    pub response_id: OpaqueId,
    pub content_fingerprint: Digest,
    pub receiver: ReceiverKey,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MemberKey {
    Execution { execution: ExecutionKey },
    Release { release: ReleaseKey },
}

impl MemberKey {
    pub fn receiver(&self) -> &ReceiverKey {
        match self {
            Self::Execution { execution } => &execution.receiver,
            Self::Release { release } => &release.receiver,
        }
    }

    pub fn validate(&self) -> ProtocolResult<()> {
        let expected = match self {
            Self::Execution { .. } => ReceiverKind::Runtime,
            Self::Release { .. } => ReceiverKind::ReleaseProducer,
        };
        if self.receiver().kind != expected {
            return Err(ProtocolError::AudienceMismatch);
        }
        Ok(())
    }
}
