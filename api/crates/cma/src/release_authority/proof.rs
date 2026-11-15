use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::{validation, *};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EvidenceAction {
    Bootstrap,
    Begin,
    ConfirmDrained,
    Prepare,
    Commit,
    Activate,
    Hold,
    Abort,
    SettleEffect,
}

/// Exact store-supplied context. A proof for another revision cannot advance it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct EvidenceRequest {
    pub deployment: String,
    pub operation_id: String,
    pub epoch: i64,
    pub control_revision: i64,
    pub action: EvidenceAction,
    pub request_sha256: String,
    pub state_sha256: String,
    pub subject: EvidenceSubject,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum EvidenceSubject {
    Bootstrap {
        request: BootstrapRequest,
    },
    Begin {
        request: Box<TransitionRequest>,
        head: Box<ReleaseHead>,
    },
    Advance {
        request: AdvanceRequest,
        head: Box<ReleaseHead>,
        operation: Box<ReleaseOperation>,
    },
    Settle {
        permit: Box<EffectPermit>,
        head: Box<ReleaseHead>,
    },
}

/// Deliberately not deserializable: request JSON cannot carry a trusted proof.
#[derive(Debug, Clone)]
pub(crate) struct VerifiedEvidence {
    request: EvidenceRequest,
    digest: String,
}

impl VerifiedEvidence {
    /// Only the trusted live verifier calls this after acquiring full evidence.
    /// A supplied hash or operator assertion is not an observation certificate.
    pub(crate) fn from_verified_observations(
        request: &EvidenceRequest,
        digest: String,
    ) -> Result<Self> {
        validation::digest(&digest)?;
        Ok(Self {
            request: request.clone(),
            digest,
        })
    }

    pub(super) fn check(self, expected: &EvidenceRequest) -> Result<String> {
        if &self.request != expected {
            return Err(AuthorityError::EvidenceRequired);
        }
        validation::digest(&self.digest)?;
        Ok(self.digest)
    }
}

/// Production implementations acquire live, complete evidence themselves.
///
/// Bootstrap includes legacy work that never held a permit. Drain/commit join
/// controller and receiver finality; ACK, process death and lease expiry do not
/// suffice. SettleEffect joins exact process incarnation, durable adoption and
/// lifecycle/cleanup terminality. Inert-read availability is separate evidence.
/// No production implementation may merely accept caller-supplied proof JSON.
#[async_trait]
pub(crate) trait EvidenceVerifier: Send + Sync {
    async fn verify(&self, request: &EvidenceRequest) -> Result<VerifiedEvidence>;
}
