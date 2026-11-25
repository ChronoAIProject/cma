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
    HandoffRecoveryDeadline,
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
            Self::HandoffRecoveryDeadline => "handoff_recovery_deadline",
        }
    }
}

enum Surface {
    Environment,
    FileRead,
    FileWrite,
}

pub(super) struct RelayDiagnostic {
    surface: Surface,
    request_hash: Arc<str>,
    hop_hash: Option<String>,
}

impl RelayDiagnostic {
    pub(super) fn from_request(request: &Request) -> Option<Self> {
        let (method, path) = (request.method(), request.uri().path());
        let (_, suffix) = path.strip_prefix("/api/v1/agents/")?.split_once('/')?;
        let surface = match (method, suffix) {
            (&axum::http::Method::GET, "environment") => Surface::Environment,
            (&axum::http::Method::POST, "files") => Surface::FileWrite,
            _ if super::file_read::eligible(path, method) => Surface::FileRead,
            _ => return None,
        };
        Some(Self {
            surface,
            request_hash: DiagnosticContext::current().request_hash,
            hop_hash: None,
        })
    }

    /// The per-process request hashes differ. The authenticated hop is a
    /// shared correlation input. Never record the hop proof itself.
    pub(super) fn correlate_hop(&mut self, proof: &[u8]) {
        self.hop_hash = Some(format!("{:x}", Sha256::digest(proof)));
    }

    pub(super) fn record(&self, reason: Refusal) {
        if matches!(self.surface, Surface::Environment) {
            tracing::info!(
                target: "cma::agent_manager::environment_relay",
                parent: None,
                request_hash = self.request_hash.as_ref(),
                hop_hash = self.hop_hash.as_deref(),
                reason = reason.as_str(),
                "Agent Environment relay refused"
            );
        } else {
            tracing::info!(
                target: "cma::agent_manager::file_relay",
                parent: None,
                request_hash = self.request_hash.as_ref(),
                hop_hash = self.hop_hash.as_deref(),
                operation = if matches!(self.surface, Surface::FileRead) { "read" } else { "write" },
                reason = reason.as_str(),
                "Agent file relay refused"
            );
        }
    }
}
