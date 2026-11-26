//! Closed ownership diagnostics: no peer text, addresses, identities or credentials.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnershipPhase {
    Challenge,
    Confirm,
    Renew,
    Revoke,
}

impl OwnershipPhase {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Challenge => "challenge",
            Self::Confirm => "confirm",
            Self::Renew => "renew",
            Self::Revoke => "revoke",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnershipFailureReason {
    MissingRoute,
    ProtocolMismatch,
    LeaseMismatch,
    HttpAuthRefused,
    HttpRefused,
    MalformedReply,
    TransportUnavailable,
    Deadline,
    AuthorityLost,
}

impl OwnershipFailureReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MissingRoute => "missing_route",
            Self::ProtocolMismatch => "protocol_mismatch",
            Self::LeaseMismatch => "lease_mismatch",
            Self::HttpAuthRefused => "http_auth_refused",
            Self::HttpRefused => "http_refused",
            Self::MalformedReply => "malformed_reply",
            Self::TransportUnavailable => "transport_unavailable",
            Self::Deadline => "deadline",
            Self::AuthorityLost => "authority_lost",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnershipFailure {
    pub phase: OwnershipPhase,
    pub reason: OwnershipFailureReason,
    pub http_status: Option<u16>,
    /// Untrusted numeric observations, never evidence that a reply was accepted.
    pub protocol: Option<u32>,
    pub lease_ms: Option<u64>,
}

impl OwnershipFailure {
    pub const fn new(phase: OwnershipPhase, reason: OwnershipFailureReason) -> Self {
        Self {
            phase,
            reason,
            http_status: None,
            protocol: None,
            lease_ms: None,
        }
    }

    pub fn with_status(mut self, status: u16) -> Self {
        self.http_status = Some(status);
        self
    }

    pub fn http(phase: OwnershipPhase, status: u16) -> Self {
        use OwnershipFailureReason::*;
        Self::new(
            phase,
            match status {
                404 => MissingRoute,
                401 | 403 => HttpAuthRefused,
                _ => HttpRefused,
            },
        )
        .with_status(status)
    }
}

pub(crate) struct OwnershipExchangeFailure {
    pub observation: OwnershipFailure,
    pub error: crate::AdapterError,
    settlement_origin: Option<crate::owner_protocol::ReceiverSettlementFailure>,
}

impl OwnershipFailure {
    pub(crate) fn with_error(self, error: crate::AdapterError) -> OwnershipExchangeFailure {
        OwnershipExchangeFailure {
            observation: self,
            error,
            settlement_origin: None,
        }
    }
}

impl From<OwnershipFailure> for OwnershipExchangeFailure {
    fn from(observation: OwnershipFailure) -> Self {
        observation.with_error(crate::AdapterError::Rejected(
            "runtime ownership negotiation failed or is unsupported".into(),
        ))
    }
}

impl OwnershipExchangeFailure {
    pub(crate) fn with_settlement_origin(
        mut self,
        origin: crate::owner_protocol::ReceiverSettlementFailure,
    ) -> Self {
        self.settlement_origin = Some(origin);
        self
    }

    pub(crate) fn settlement_failure(&self) -> crate::owner_protocol::ReceiverSettlementFailure {
        use crate::owner_protocol::ReceiverSettlementFailure as Failure;
        if let Some(origin) = self.settlement_origin {
            return origin;
        }
        match self.observation.reason {
            OwnershipFailureReason::Deadline => Failure::Deadline,
            OwnershipFailureReason::ProtocolMismatch | OwnershipFailureReason::LeaseMismatch => {
                Failure::IncompatibleProtocol
            }
            OwnershipFailureReason::AuthorityLost => Failure::AuthorityLost,
            OwnershipFailureReason::MalformedReply
            | OwnershipFailureReason::HttpAuthRefused
            | OwnershipFailureReason::HttpRefused
            | OwnershipFailureReason::MissingRoute => Failure::Invalid,
            OwnershipFailureReason::TransportUnavailable => Failure::Unclassified,
        }
    }
}
