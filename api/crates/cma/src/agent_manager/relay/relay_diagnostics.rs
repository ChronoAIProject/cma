use axum::extract::Request;
use axum::http::StatusCode;
use layer2_core::diagnostic_context::DiagnosticContext;
use sha2::{Digest, Sha256};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Refusal {
    AdmissionClosed,
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
            Self::AdmissionClosed => "admission_closed",
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

#[derive(Clone, Copy)]
pub(super) enum RecoveryTrigger {
    PeerConnectionFailed,
    PeerOwnerHandoff,
    VerifiedWriteNotEntered,
}

impl RecoveryTrigger {
    const fn as_str(self) -> &'static str {
        match self {
            Self::PeerConnectionFailed => "peer_connection_failed",
            Self::PeerOwnerHandoff => "peer_owner_handoff",
            Self::VerifiedWriteNotEntered => "verified_write_not_entered",
        }
    }
}

pub(super) struct RelayDiagnostic {
    surface: Surface,
    request_hash: Arc<str>,
    hop_hash: Option<String>,
}

pub(super) fn peer_write_entered(request: &Request, epoch: u64) {
    if request
        .extensions()
        .get::<super::http::VerifiedPeer>()
        .is_none()
    {
        return;
    }
    let operation = match (request.method().as_str(), request.uri().path()) {
        ("PATCH", path) if path.starts_with("/api/v1/agents/") => match path.rsplit('/').next() {
            Some("preferences") => "preferences",
            Some("settings") => "settings",
            _ => return,
        },
        ("POST", path) if path.starts_with("/api/v1/agents/") && path.ends_with("/skills") => {
            "skill_install"
        }
        ("POST", path) if path.starts_with("/api/v2/agents/") && path.ends_with("/controls") => {
            "response_control"
        }
        _ => return,
    };
    let Some(request_id) = layer1_public::rest::request_id_from_headers(request.headers()) else {
        return;
    };
    // A random request UUID can be correlated across the trusted relay hop
    // without exposing client-provided identifiers, resource IDs or payloads.
    if !uuid::Uuid::parse_str(request_id).is_ok_and(|id| id.get_version_num() == 4) {
        return;
    }
    tracing::debug!(
        target: "cma::agent_manager::peer_write",
        parent: None,
        request_sha256 = %format!("{:x}", Sha256::digest(request_id)),
        operation,
        epoch,
        "verified peer write entered owner scope"
    );
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

    pub(super) fn recovery(&self, trigger: RecoveryTrigger, status: Option<StatusCode>) {
        let outcome = match status {
            None => "started",
            Some(status) if status.is_success() => "recovered",
            Some(_) => "refused",
        };
        tracing::info!(
            target: "cma::agent_manager::file_relay",
            parent: None,
            request_hash = self.request_hash.as_ref(),
            hop_hash = self.hop_hash.as_deref(),
            operation = if matches!(self.surface, Surface::FileRead) { "read" } else { "write" },
            trigger = trigger.as_str(),
            outcome,
            status = status.map(|status| status.as_u16()),
            "Agent file relay handoff recovery"
        );
    }
}
