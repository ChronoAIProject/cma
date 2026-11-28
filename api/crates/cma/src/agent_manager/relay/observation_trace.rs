//! Opt-in branch telemetry for synthetic observation-open campaigns.
use super::{http, ingress, OwnershipToken, Relay, PROOF};
use axum::{
    extract::Request,
    http::{Method, StatusCode},
    response::Response,
};
use sha2::{Digest, Sha256};
use std::panic::Location;

#[derive(Clone, Copy)]
pub(super) enum Decision {
    ServiceMissing,
    CoordinatorNotReady,
    BodyBudget,
    CommandServiceMissing,
    IncarnationMissing,
    DestinationUnavailable,
    DestinationDeadline,
    PeerOwnerMismatch,
    MembershipUnavailable,
    PeerReachedNonowner,
    SigningFailed,
    PeerTransportFailed,
    PeerAdmissionDeadline,
    AdmissionDecodeFailed,
}

impl Decision {
    const fn label(self) -> &'static str {
        match self {
            Self::ServiceMissing => "observation_service_missing",
            Self::CoordinatorNotReady => "coordinator_not_ready",
            Self::BodyBudget => "body_budget_unavailable",
            Self::CommandServiceMissing => "response_command_service_missing",
            Self::IncarnationMissing => "agent_incarnation_missing",
            Self::DestinationUnavailable => "destination_unavailable",
            Self::DestinationDeadline => "destination_deadline",
            Self::PeerOwnerMismatch => "peer_owner_mismatch",
            Self::MembershipUnavailable => "membership_unavailable",
            Self::PeerReachedNonowner => "peer_reached_nonowner",
            Self::SigningFailed => "peer_signing_failed",
            Self::PeerTransportFailed => "peer_transport_failed",
            Self::PeerAdmissionDeadline => "peer_admission_deadline",
            Self::AdmissionDecodeFailed => "admission_decode_failed",
        }
    }
}

pub(super) struct OpenTrace {
    request_hash: sha2::digest::Output<Sha256>,
    instance: Option<String>,
    has_peer_proof: bool,
    pub(super) peer_verified: bool,
}

impl OpenTrace {
    pub(super) fn from_request(relay: &Relay, request: &Request) -> Option<Self> {
        if relay.observation_open_trace.get().is_none()
            || !tracing::enabled!(target: "cma::agent_manager::observation_open", tracing::Level::DEBUG)
        {
            return None;
        }
        let id = request
            .uri()
            .path()
            .strip_prefix("/api/v2/responses/")?
            .strip_suffix("/events")?;
        if request.method() != Method::GET || id.is_empty() || id.contains('/') {
            return None;
        }
        let request_id = layer1_public::rest::request_id_from_headers(ingress::headers(request))?;
        Some(Self {
            request_hash: Sha256::digest(request_id.as_bytes()),
            instance: alias(&relay.coordinator.instance_id).map(str::to_owned),
            has_peer_proof: request.headers().contains_key(PROOF),
            peer_verified: false,
        })
    }

    #[track_caller]
    pub(super) fn opened(&self, response: &Response, owner: &OwnershipToken) {
        if !self.has_peer_proof
            && response.status() == StatusCode::OK
            && response
                .headers()
                .get(axum::http::header::CONTENT_TYPE)
                .is_some_and(|value| value == "text/event-stream")
        {
            self.record("opened", "sse_opened", StatusCode::OK, Some(owner), None);
        }
    }

    #[track_caller]
    pub(super) fn forwarded(&self, response: &Response, owner: &OwnershipToken) {
        if response.status() == StatusCode::SERVICE_UNAVAILABLE {
            // The status alone cannot identify the peer's public problem code.
            self.record(
                "forwarded",
                "peer_http_503",
                response.status(),
                Some(owner),
                None,
            );
        } else {
            self.opened(response, owner);
        }
    }

    #[track_caller]
    fn record(
        &self,
        outcome: &'static str,
        branch: &'static str,
        status: StatusCode,
        owner: Option<&OwnershipToken>,
        signed_epoch: Option<u64>,
    ) {
        let location = Location::caller();
        tracing::debug!(
            target: "cma::agent_manager::observation_open",
            parent: None,
            request_sha256 = %format_args!("{:x}", self.request_hash),
            hop = if self.has_peer_proof { "receiving_peer" } else { "ingress" },
            peer_verified = self.peer_verified,
            instance = self.instance.as_deref(),
            outcome,
            branch,
            source_file = location.file(),
            source_line = location.line(),
            http_status = status.as_u16(),
            read_owner = owner.and_then(|token| alias(&token.instance_id)),
            read_epoch = owner.map(|token| token.epoch),
            signed_epoch,
            "observation open"
        );
    }
}

fn alias(instance: &str) -> Option<&str> {
    (!instance.is_empty()
        && instance.len() <= 128
        && instance
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')))
    .then_some(instance)
}

#[track_caller]
pub(super) fn refuse(
    trace: Option<&OpenTrace>,
    decision: Decision,
    owner: Option<&OwnershipToken>,
    signed_epoch: Option<u64>,
) -> Response {
    if let Some(trace) = trace {
        trace.record(
            "refused",
            decision.label(),
            StatusCode::SERVICE_UNAVAILABLE,
            owner,
            signed_epoch,
        );
    }
    http::unavailable()
}
