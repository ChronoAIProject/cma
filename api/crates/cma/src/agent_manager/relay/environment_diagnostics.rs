use axum::extract::Request;
use layer2_core::diagnostic_context::DiagnosticContext;
use sha2::{Digest, Sha256};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Refusal {
    CoordinatorNotReady,
    AuthorizationUnavailable,
    MembershipUnavailable,
    OwnerLookupUnavailable,
    OwnerMissing,
    OwnerMemberUnavailable,
    AuthorityChanged,
    PeerSignature,
    PeerTransport,
    PeerResponse,
}

impl Refusal {
    const fn as_str(self) -> &'static str {
        match self {
            Self::CoordinatorNotReady => "coordinator_not_ready",
            Self::AuthorizationUnavailable => "authorization_unavailable",
            Self::MembershipUnavailable => "membership_unavailable",
            Self::OwnerLookupUnavailable => "owner_lookup_unavailable",
            Self::OwnerMissing => "owner_missing",
            Self::OwnerMemberUnavailable => "owner_member_unavailable",
            Self::AuthorityChanged => "authority_changed",
            Self::PeerSignature => "peer_signature",
            Self::PeerTransport => "peer_transport",
            Self::PeerResponse => "peer_response",
        }
    }
}

pub(super) struct EnvironmentRead {
    request_hash: Arc<str>,
    hop_hash: Option<String>,
}

impl EnvironmentRead {
    pub(super) fn from_request(request: &Request) -> Option<Self> {
        (request.method() == axum::http::Method::GET
            && request
                .uri()
                .path()
                .strip_prefix("/api/v1/agents/")
                .and_then(|path| path.split_once('/'))
                .is_some_and(|(_, suffix)| suffix == "environment"))
        .then(|| Self {
            request_hash: DiagnosticContext::current().request_hash,
            hop_hash: None,
        })
    }

    /// The per-process request hashes differ. The signed hop already carries a
    /// single-use nonce, so the peers can correlate without a new wire header.
    /// Only pass a proof generated for this hop or authenticated by the receiver.
    pub(super) fn correlate_hop(&mut self, proof: &[u8]) {
        self.hop_hash = Some(format!("{:x}", Sha256::digest(proof)));
    }

    pub(super) fn record(&self, reason: Refusal) {
        tracing::info!(
            target: "cma::agent_manager::environment_relay",
            parent: None,
            request_hash = self.request_hash.as_ref(),
            hop_hash = self.hop_hash.as_deref(),
            reason = reason.as_str(),
            "Agent Environment relay refused"
        );
    }
}
