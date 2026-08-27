//! MongoDB's durable session-runtime coordination state machine.
//!
//! Current authority and the active lease live with the session document, so a
//! runtime-handle change is one document write. Long-lived idempotency receipts
//! and cleanup debt live in a separate collection, avoiding an ever-growing
//! session document. Every command below is applied inside one Mongo transaction.

use std::{
    collections::{BTreeMap, BTreeSet},
    num::NonZeroU64,
};

use futures_util::{FutureExt, TryStreamExt};
use layer2_core::{
    ConversationLink, ConversationOpeningLease, ConversationOpeningLeaseRequest,
    ConversationWriteAuthority, Event, EventKind, RuntimeAuthority, RuntimeAuthorityRetirement,
    RuntimeAuthoritySwitch, RuntimeCandidate, RuntimeCleanup, RuntimeCleanupReason,
    RuntimeReservation, SessionBoundary, SessionInteractionLeaseRequest, SessionLease,
    SessionLeaseRequest, SessionOperation, StoredEventKind,
};
use mongodb::bson::{doc, Bson, DateTime, Document};
use mongodb::options::{ReadConcern, ReplaceOptions, WriteConcern};
use serde::{Deserialize, Serialize};

use super::{parse_plain, serde_plain_string, MongoSessionStore, SessionDoc, StoreError};

pub(super) const COORDINATION_RECEIPTS: &str = "session_coordination_receipts";

const RECEIPT_LEASE: &str = "lease";
const RECEIPT_RESERVATION: &str = "reservation";
const RECEIPT_RETIREMENT: &str = "retirement";
const RECEIPT_CLEANUP: &str = "cleanup";
const RUNTIME_STARTED_MARKER_AUDIT_BATCH_SIZE: usize = 128;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct CoordinationDoc {
    #[serde(default)]
    authority_generation: i64,
    #[serde(default)]
    next_generation: i64,
    #[serde(default)]
    receipt_count: i64,
    /// Monotonic source of bounded interaction fencing tokens.
    #[serde(default)]
    interaction_epoch: i64,
    /// Monotonic identity of the published inbound conversation link.
    #[serde(default)]
    conversation_epoch: i64,
    /// Opening token promoted into the current published link.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    conversation_link_token: Option<String>,
    /// Replacement opening that has authority to publish, but has not yet
    /// displaced the current link.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    conversation_opening: Option<ConversationOpeningDoc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lease: Option<LeaseDoc>,
    /// Refreshed with MongoDB's `$currentDate` at the start of every command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    server_now: Option<DateTime>,
}

/// Minimal session shape needed to read runtime authority.
///
/// Keeping this distinct from `SessionDoc` makes it impossible for the
/// high-frequency authority read to deserialize staged skill bodies by
/// accident.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct RuntimeAuthorityProjectionDoc {
    #[serde(rename = "_id")]
    id: String,
    #[serde(rename = "_cma_write_protocol", default)]
    write_protocol: i32,
    #[serde(rename = "_cma_coordination_protocol", default)]
    coordination_protocol: i32,
    #[serde(default)]
    runtime_id: Option<String>,
    #[serde(default)]
    coordination: Option<CoordinationDoc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct BoundaryDoc {
    status: String,
    generation: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    runtime_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct LeaseDoc {
    holder: String,
    token: String,
    /// Retry nonce retained only for the one active bounded interaction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    interaction_nonce: Option<String>,
    boundary: BoundaryDoc,
    operation: String,
    expires_at: DateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ConversationOpeningDoc {
    holder: String,
    token: String,
    runtime_id: String,
    #[serde(default)]
    runtime_generation: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    runtime_candidate_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session_lease_token: Option<String>,
    expires_at: DateTime,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct LeaseTokenReceiptDoc {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    released: Option<LeaseReleaseDoc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct LeaseReleaseDoc {
    holder: String,
    boundary: BoundaryDoc,
    operation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CleanupDoc {
    generation: i64,
    runtime_id: String,
    reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct AuthoritySwitchDoc {
    revision: i64,
    #[serde(default)]
    runtime_started_seq: i64,
    boundary: BoundaryDoc,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cleanup: Option<CleanupDoc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct AuthorityRetirementDoc {
    revision: i64,
    boundary: BoundaryDoc,
    cleanup: CleanupDoc,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ReservationReceiptDoc {
    lease_token: String,
    generation: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    runtime_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    switch: Option<AuthoritySwitchDoc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct RetirementReceiptDoc {
    lease_token: String,
    original_boundary: BoundaryDoc,
    outcome: AuthorityRetirementDoc,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CleanupReceiptDoc {
    cleanup: CleanupDoc,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    completed_by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(super) struct CoordinationReceiptIdDoc {
    session_id: String,
    kind: String,
    token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
enum CoordinationReceiptPayloadDoc {
    LeaseToken(LeaseTokenReceiptDoc),
    Reservation(ReservationReceiptDoc),
    Retirement(RetirementReceiptDoc),
    Cleanup(CleanupReceiptDoc),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct CoordinationReceiptDoc {
    #[serde(rename = "_id")]
    id: CoordinationReceiptIdDoc,
    #[serde(rename = "_cma_write_protocol")]
    write_protocol: i32,
    #[serde(rename = "_cma_coordination_protocol")]
    coordination_protocol: i32,
    payload: CoordinationReceiptPayloadDoc,
}

#[derive(Debug, Clone)]
pub(super) enum CoordinationCommand {
    Acquire(SessionLeaseRequest),
    AcquireInteraction(SessionInteractionLeaseRequest),
    Renew {
        lease: SessionLease,
        ttl: std::time::Duration,
    },
    RenewInteraction {
        lease: SessionLease,
        ttl: std::time::Duration,
    },
    Advance {
        lease: SessionLease,
        boundary: SessionBoundary,
    },
    Release(SessionLease),
    AcquireConversationOpening {
        request: ConversationOpeningLeaseRequest,
        session_lease: Option<SessionLease>,
    },
    RenewConversationOpening {
        opening: ConversationOpeningLease,
        session_lease: Option<SessionLease>,
        ttl: std::time::Duration,
    },
    PublishConversationLink {
        opening: ConversationOpeningLease,
        session_lease: Option<SessionLease>,
    },
    ReleaseConversationOpening(ConversationOpeningLease),
    Reserve {
        lease: SessionLease,
        token: String,
    },
    Register {
        lease: SessionLease,
        reservation: RuntimeReservation,
        runtime_id: String,
    },
    Switch {
        lease: SessionLease,
        candidate: RuntimeCandidate,
    },
    Retire {
        lease: SessionLease,
        token: String,
    },
    Acknowledge {
        lease: SessionLease,
        cleanup: RuntimeCleanup,
    },
}

#[derive(Debug)]
pub(super) enum CoordinationOutcome {
    Lease(SessionLease),
    Reservation(RuntimeReservation),
    Candidate(RuntimeCandidate),
    Switch(RuntimeAuthoritySwitch),
    Retirement(RuntimeAuthorityRetirement),
    ConversationOpening(ConversationOpeningLease),
    ConversationLink(ConversationLink),
    Unit,
}

#[derive(Debug)]
enum ReceiptMutation {
    Insert(CoordinationReceiptDoc),
    Replace(CoordinationReceiptDoc),
    Delete(CoordinationReceiptIdDoc),
}

impl BoundaryDoc {
    fn from_core(boundary: &SessionBoundary) -> Result<Self, StoreError> {
        Ok(Self {
            status: serde_plain_string(&boundary.status)?,
            generation: stored_generation(boundary.authority.generation)?,
            runtime_id: boundary.authority.runtime_id.clone(),
        })
    }

    fn to_core(&self) -> Result<SessionBoundary, StoreError> {
        Ok(SessionBoundary {
            status: parse_plain(&self.status)?,
            authority: RuntimeAuthority {
                generation: loaded_generation(self.generation)?,
                runtime_id: self.runtime_id.clone(),
            },
        })
    }
}

impl LeaseDoc {
    fn from_request(
        request: &SessionLeaseRequest,
        expires_at: DateTime,
    ) -> Result<Self, StoreError> {
        if request.holder.is_empty() || request.token.is_empty() {
            return Err(StoreError::Malformed(
                "session lease holder and token must not be empty".into(),
            ));
        }
        Ok(Self {
            holder: request.holder.clone(),
            token: request.token.clone(),
            interaction_nonce: None,
            boundary: BoundaryDoc::from_core(&request.boundary)?,
            operation: operation_name(request.operation).into(),
            expires_at,
        })
    }

    fn from_interaction(
        request: &SessionInteractionLeaseRequest,
        token: String,
        expires_at: DateTime,
    ) -> Result<Self, StoreError> {
        if request.holder.is_empty() || request.nonce.is_empty() {
            return Err(StoreError::Malformed(
                "session interaction lease holder and nonce must not be empty".into(),
            ));
        }
        Ok(Self {
            holder: request.holder.clone(),
            token,
            interaction_nonce: Some(request.nonce.clone()),
            boundary: BoundaryDoc::from_core(&request.boundary)?,
            operation: operation_name(SessionOperation::Interact).into(),
            expires_at,
        })
    }

    fn to_core(&self, id: &layer2_core::SessionId) -> Result<SessionLease, StoreError> {
        if self.holder.is_empty() || self.token.is_empty() {
            return Err(StoreError::Malformed(
                "stored session lease holder and token must not be empty".into(),
            ));
        }
        let operation = parse_operation(&self.operation)?;
        if (operation == SessionOperation::Interact) != self.interaction_nonce.is_some() {
            return Err(StoreError::Malformed(
                "stored interaction lease nonce and operation disagree".into(),
            ));
        }
        if self.interaction_nonce.as_deref().is_some_and(str::is_empty) {
            return Err(StoreError::Malformed(
                "stored interaction lease nonce must not be empty".into(),
            ));
        }
        Ok(SessionLease {
            session_id: id.clone(),
            holder: self.holder.clone(),
            token: self.token.clone(),
            boundary: self.boundary.to_core()?,
            operation,
        })
    }

    fn matches_request(&self, request: &SessionLeaseRequest) -> Result<bool, StoreError> {
        Ok(self.holder == request.holder
            && self.token == request.token
            && self.boundary == BoundaryDoc::from_core(&request.boundary)?
            && parse_operation(&self.operation)? == request.operation)
    }

    fn matches_lease(&self, lease: &SessionLease) -> Result<bool, StoreError> {
        Ok(self.holder == lease.holder
            && self.token == lease.token
            && self.boundary == BoundaryDoc::from_core(&lease.boundary)?
            && parse_operation(&self.operation)? == lease.operation)
    }

    fn matches_interaction_request(
        &self,
        request: &SessionInteractionLeaseRequest,
    ) -> Result<bool, StoreError> {
        Ok(self.holder == request.holder
            && self.interaction_nonce.as_deref() == Some(request.nonce.as_str())
            && self.boundary == BoundaryDoc::from_core(&request.boundary)?
            && parse_operation(&self.operation)? == SessionOperation::Interact)
    }

    fn same_identity(&self, lease: &SessionLease) -> Result<bool, StoreError> {
        Ok(self.holder == lease.holder
            && self.token == lease.token
            && parse_operation(&self.operation)? == lease.operation)
    }
}

impl ConversationOpeningDoc {
    fn from_request(
        request: &ConversationOpeningLeaseRequest,
        session_lease: Option<&SessionLease>,
        expires_at: DateTime,
    ) -> Result<Self, StoreError> {
        if request.holder.is_empty() || request.token.is_empty() || request.runtime_id.is_empty() {
            return Err(StoreError::Malformed(
                "conversation opening holder, token, and runtime handle must not be empty".into(),
            ));
        }
        Ok(Self {
            holder: request.holder.clone(),
            token: request.token.clone(),
            runtime_id: request.runtime_id.clone(),
            runtime_generation: stored_generation(request.runtime_generation)?,
            runtime_candidate_token: request.runtime_candidate_token.clone(),
            session_lease_token: session_lease.map(|lease| lease.token.clone()),
            expires_at,
        })
    }

    fn to_core(&self, id: &layer2_core::SessionId) -> Result<ConversationOpeningLease, StoreError> {
        if self.holder.is_empty() || self.token.is_empty() || self.runtime_id.is_empty() {
            return Err(StoreError::Malformed(
                "stored conversation opening holder, token, and runtime handle must not be empty"
                    .into(),
            ));
        }
        if self
            .session_lease_token
            .as_deref()
            .is_some_and(str::is_empty)
        {
            return Err(StoreError::Malformed(
                "stored conversation opening lease token must not be empty".into(),
            ));
        }
        if self
            .runtime_candidate_token
            .as_deref()
            .is_some_and(str::is_empty)
        {
            return Err(StoreError::Malformed(
                "stored conversation opening candidate token must not be empty".into(),
            ));
        }
        Ok(ConversationOpeningLease {
            session_id: id.clone(),
            holder: self.holder.clone(),
            token: self.token.clone(),
            runtime_id: self.runtime_id.clone(),
            runtime_generation: loaded_generation(self.runtime_generation)?,
            runtime_candidate_token: self.runtime_candidate_token.clone(),
            session_lease_token: self.session_lease_token.clone(),
        })
    }

    fn matches(&self, opening: &ConversationOpeningLease) -> Result<bool, StoreError> {
        Ok(self.to_core(&opening.session_id)? == *opening)
    }
}

impl LeaseReleaseDoc {
    fn from_core(lease: &SessionLease) -> Result<Self, StoreError> {
        Ok(Self {
            holder: lease.holder.clone(),
            boundary: BoundaryDoc::from_core(&lease.boundary)?,
            operation: operation_name(lease.operation).into(),
        })
    }

    fn matches_lease(&self, lease: &SessionLease) -> Result<bool, StoreError> {
        Ok(self.holder == lease.holder
            && self.boundary == BoundaryDoc::from_core(&lease.boundary)?
            && parse_operation(&self.operation)? == lease.operation)
    }

    fn validate_stored(&self) -> Result<(), StoreError> {
        if self.holder.is_empty() {
            return Err(StoreError::Malformed(
                "MongoDB released lease receipt has an empty holder".into(),
            ));
        }
        self.boundary.to_core()?;
        parse_operation(&self.operation)?;
        Ok(())
    }
}

impl CleanupDoc {
    fn from_core(cleanup: &RuntimeCleanup) -> Result<Self, StoreError> {
        if cleanup.runtime_id.is_empty() || cleanup.token.is_empty() {
            return Err(StoreError::Malformed(
                "runtime cleanup handle and token must not be empty".into(),
            ));
        }
        Ok(Self {
            generation: stored_generation(cleanup.generation)?,
            runtime_id: cleanup.runtime_id.clone(),
            reason: cleanup_reason_name(cleanup.reason).into(),
        })
    }

    fn to_core(
        &self,
        id: &layer2_core::SessionId,
        token: String,
    ) -> Result<RuntimeCleanup, StoreError> {
        if self.runtime_id.is_empty() || token.is_empty() {
            return Err(StoreError::Malformed(
                "stored runtime cleanup handle and token must not be empty".into(),
            ));
        }
        Ok(RuntimeCleanup {
            session_id: id.clone(),
            generation: loaded_generation(self.generation)?,
            runtime_id: self.runtime_id.clone(),
            token,
            reason: parse_cleanup_reason(&self.reason)?,
        })
    }
}

impl CoordinationReceiptIdDoc {
    fn new(id: &layer2_core::SessionId, kind: &str, token: &str) -> Self {
        Self {
            session_id: id.0.clone(),
            kind: kind.into(),
            token: token.into(),
        }
    }

    fn filter(&self) -> Document {
        doc! {
            "_id.session_id": &self.session_id,
            "_id.kind": &self.kind,
            "_id.token": &self.token,
        }
    }
}

impl CoordinationReceiptDoc {
    fn new(id: CoordinationReceiptIdDoc, payload: CoordinationReceiptPayloadDoc) -> Self {
        Self {
            id,
            write_protocol: super::WRITE_PROTOCOL,
            coordination_protocol: super::COORDINATION_RECEIPT_PROTOCOL,
            payload,
        }
    }

    fn validate_id(&self, expected: &CoordinationReceiptIdDoc) -> Result<(), StoreError> {
        self.validate_stored()?;
        if &self.id != expected {
            return Err(StoreError::Malformed(
                "MongoDB coordination receipt identity or protocol changed".into(),
            ));
        }
        Ok(())
    }

    fn validate_stored(&self) -> Result<(), StoreError> {
        if self.write_protocol != super::WRITE_PROTOCOL
            || self.coordination_protocol != super::COORDINATION_RECEIPT_PROTOCOL
            || self.id.session_id.is_empty()
            || self.id.token.is_empty()
        {
            return Err(StoreError::Malformed(
                "MongoDB coordination receipt identity or protocol is invalid".into(),
            ));
        }
        let id = layer2_core::SessionId(self.id.session_id.clone());
        match (&self.payload, self.id.kind.as_str()) {
            (CoordinationReceiptPayloadDoc::LeaseToken(stored), RECEIPT_LEASE) => {
                if let Some(released) = stored.released.as_ref() {
                    released.validate_stored()?;
                }
                Ok(())
            }
            (CoordinationReceiptPayloadDoc::Reservation(stored), RECEIPT_RESERVATION) => {
                if stored.lease_token.is_empty() {
                    return Err(StoreError::Malformed(
                        "MongoDB reservation receipt has an empty lease token".into(),
                    ));
                }
                let generation = NonZeroU64::new(loaded_generation(stored.generation)?)
                    .ok_or_else(|| {
                        StoreError::Malformed(
                            "MongoDB reservation receipt generation is zero".into(),
                        )
                    })?;
                let Some(runtime_id) = stored.runtime_id.as_deref() else {
                    if stored.switch.is_some() {
                        return Err(StoreError::Malformed(
                            "MongoDB unregistered reservation carries a completed switch".into(),
                        ));
                    }
                    return Ok(());
                };
                if runtime_id.is_empty() {
                    return Err(StoreError::Malformed(
                        "MongoDB reservation receipt has an empty runtime handle".into(),
                    ));
                }
                if let Some(completed) = stored.switch.as_ref() {
                    loaded_generation(completed.revision)?;
                    if loaded_generation(completed.runtime_started_seq)? == 0 {
                        return Err(StoreError::Malformed(
                            "MongoDB completed switch has no runtime-start marker".into(),
                        ));
                    }
                    let boundary = completed.boundary.to_core()?;
                    if boundary.authority.generation != generation.get()
                        || boundary.authority.runtime_id.as_deref() != Some(runtime_id)
                    {
                        return Err(StoreError::Malformed(
                            "MongoDB completed switch disagrees with its reservation".into(),
                        ));
                    }
                    if let Some(cleanup) = completed.cleanup.as_ref() {
                        let cleanup =
                            cleanup.to_core(&id, retired_cleanup_token(&self.id.token))?;
                        if cleanup.reason != RuntimeCleanupReason::RetiredAuthority {
                            return Err(StoreError::Malformed(
                                "MongoDB completed switch carries a non-retirement cleanup".into(),
                            ));
                        }
                    }
                }
                Ok(())
            }
            (CoordinationReceiptPayloadDoc::Retirement(stored), RECEIPT_RETIREMENT) => {
                if stored.lease_token.is_empty() {
                    return Err(StoreError::Malformed(
                        "MongoDB retirement receipt has an empty lease token".into(),
                    ));
                }
                let original = stored.original_boundary.to_core()?;
                loaded_generation(stored.outcome.revision)?;
                let outcome = stored.outcome.boundary.to_core()?;
                let cleanup = stored
                    .outcome
                    .cleanup
                    .to_core(&id, retired_cleanup_token(&self.id.token))?;
                if cleanup.reason != RuntimeCleanupReason::RetiredAuthority
                    || original.status != outcome.status
                    || original.authority.generation != cleanup.generation
                    || original.authority.runtime_id.as_deref() != Some(cleanup.runtime_id.as_str())
                    || outcome.authority.generation != cleanup.generation
                    || outcome.authority.runtime_id.is_some()
                {
                    return Err(StoreError::Malformed(
                        "MongoDB retirement receipt carries inconsistent boundaries".into(),
                    ));
                }
                Ok(())
            }
            (CoordinationReceiptPayloadDoc::Cleanup(stored), RECEIPT_CLEANUP) => {
                stored.cleanup.to_core(&id, self.id.token.clone())?;
                if stored.completed_by.as_deref().is_some_and(str::is_empty) {
                    return Err(StoreError::Malformed(
                        "MongoDB cleanup receipt has an empty completion token".into(),
                    ));
                }
                Ok(())
            }
            _ => Err(StoreError::Malformed(
                "MongoDB coordination receipt kind and payload disagree".into(),
            )),
        }
    }
}

impl CoordinationCommand {
    fn id(&self) -> &layer2_core::SessionId {
        match self {
            Self::Acquire(request) => &request.session_id,
            Self::AcquireInteraction(request) => &request.session_id,
            Self::AcquireConversationOpening { request, .. } => &request.session_id,
            Self::RenewConversationOpening { opening, .. }
            | Self::PublishConversationLink { opening, .. }
            | Self::ReleaseConversationOpening(opening) => &opening.session_id,
            Self::Renew { lease, .. }
            | Self::RenewInteraction { lease, .. }
            | Self::Advance { lease, .. }
            | Self::Release(lease)
            | Self::Reserve { lease, .. }
            | Self::Register { lease, .. }
            | Self::Switch { lease, .. }
            | Self::Retire { lease, .. }
            | Self::Acknowledge { lease, .. } => &lease.session_id,
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Self::Acquire(_) => "acquire session lease",
            Self::AcquireInteraction(_) => "acquire interaction lease",
            Self::Renew { .. } => "renew session lease",
            Self::RenewInteraction { .. } => "renew interaction lease",
            Self::Advance { .. } => "advance session lease",
            Self::Release(_) => "release session lease",
            Self::AcquireConversationOpening { .. } => "acquire conversation opening",
            Self::RenewConversationOpening { .. } => "renew conversation opening",
            Self::PublishConversationLink { .. } => "publish conversation link",
            Self::ReleaseConversationOpening(_) => "release conversation opening",
            Self::Reserve { .. } => "reserve runtime generation",
            Self::Register { .. } => "register runtime candidate",
            Self::Switch { .. } => "switch runtime authority",
            Self::Retire { .. } => "retire runtime authority",
            Self::Acknowledge { .. } => "acknowledge runtime cleanup",
        }
    }

    fn receipt_ids(&self) -> Vec<CoordinationReceiptIdDoc> {
        let id = self.id();
        match self {
            Self::Acquire(request) => {
                vec![CoordinationReceiptIdDoc::new(
                    id,
                    RECEIPT_LEASE,
                    &request.token,
                )]
            }
            Self::Reserve { token, .. } => {
                vec![CoordinationReceiptIdDoc::new(
                    id,
                    RECEIPT_RESERVATION,
                    token,
                )]
            }
            Self::Register { reservation, .. } => vec![
                CoordinationReceiptIdDoc::new(id, RECEIPT_RESERVATION, &reservation.token),
                CoordinationReceiptIdDoc::new(
                    id,
                    RECEIPT_CLEANUP,
                    &candidate_cleanup_token(&reservation.token),
                ),
            ],
            Self::Switch { candidate, .. } => vec![
                CoordinationReceiptIdDoc::new(id, RECEIPT_RESERVATION, &candidate.token),
                CoordinationReceiptIdDoc::new(
                    id,
                    RECEIPT_CLEANUP,
                    &candidate_cleanup_token(&candidate.token),
                ),
                CoordinationReceiptIdDoc::new(
                    id,
                    RECEIPT_CLEANUP,
                    &retired_cleanup_token(&candidate.token),
                ),
            ],
            Self::Retire { token, .. } => vec![
                CoordinationReceiptIdDoc::new(id, RECEIPT_RETIREMENT, token),
                CoordinationReceiptIdDoc::new(id, RECEIPT_CLEANUP, &retired_cleanup_token(token)),
            ],
            Self::Acknowledge { cleanup, .. } => vec![CoordinationReceiptIdDoc::new(
                id,
                RECEIPT_CLEANUP,
                &cleanup.token,
            )],
            Self::AcquireConversationOpening { request, .. } => request
                .runtime_candidate_token
                .as_ref()
                .map(|token| {
                    vec![CoordinationReceiptIdDoc::new(
                        id,
                        RECEIPT_RESERVATION,
                        token,
                    )]
                })
                .unwrap_or_default(),
            Self::Release(lease) if lease.operation != SessionOperation::Interact => {
                vec![CoordinationReceiptIdDoc::new(
                    id,
                    RECEIPT_LEASE,
                    &lease.token,
                )]
            }
            Self::AcquireInteraction(_)
            | Self::RenewConversationOpening { .. }
            | Self::PublishConversationLink { .. }
            | Self::ReleaseConversationOpening(_)
            | Self::Release(_)
            | Self::Renew { .. }
            | Self::RenewInteraction { .. }
            | Self::Advance { .. } => Vec::new(),
        }
    }
}

pub(super) fn runtime_authority_from_doc(
    session: &SessionDoc,
) -> Result<RuntimeAuthority, StoreError> {
    validate_coordination_state(session)?;
    Ok(RuntimeAuthority {
        generation: session
            .coordination
            .as_ref()
            .map_or(Ok(0), |state| loaded_generation(state.authority_generation))?,
        runtime_id: session.runtime_id.clone(),
    })
}

pub(super) fn runtime_authority_from_projection(
    session: &RuntimeAuthorityProjectionDoc,
) -> Result<RuntimeAuthority, StoreError> {
    validate_coordination_parts(
        &session.id,
        session.write_protocol,
        session.coordination_protocol,
        session.coordination.as_ref(),
    )?;
    Ok(RuntimeAuthority {
        generation: session
            .coordination
            .as_ref()
            .map_or(Ok(0), |state| loaded_generation(state.authority_generation))?,
        runtime_id: session.runtime_id.clone(),
    })
}

pub(super) fn validate_coordination_state(session: &SessionDoc) -> Result<(), StoreError> {
    validate_coordination_parts(
        &session.id,
        session.write_protocol,
        session.coordination_protocol,
        session.coordination.as_ref(),
    )
}

fn validate_coordination_parts(
    id: &str,
    write_protocol: i32,
    coordination_protocol: i32,
    coordination: Option<&CoordinationDoc>,
) -> Result<(), StoreError> {
    if write_protocol != super::SESSION_WRITE_PROTOCOL {
        return Err(StoreError::Malformed(format!(
            "session {} carries an unknown write protocol",
            id
        )));
    }
    if coordination_protocol != super::SESSION_COORDINATION_PROTOCOL {
        return Err(StoreError::Malformed(format!(
            "session {} carries an unknown coordination protocol",
            id
        )));
    }
    let Some(state) = coordination else {
        return Ok(());
    };
    let generation = loaded_generation(state.authority_generation)?;
    let next = loaded_generation(state.next_generation)?;
    loaded_generation(state.receipt_count)?;
    let interaction_epoch = loaded_generation(state.interaction_epoch)?;
    let conversation_epoch = loaded_generation(state.conversation_epoch)?;
    match (conversation_epoch, state.conversation_link_token.as_deref()) {
        (_, None) => {}
        (0, Some(_)) => {
            return Err(StoreError::Malformed(format!(
                "session {} conversation epoch and link token disagree",
                id
            )))
        }
        (_, Some("")) => {
            return Err(StoreError::Malformed(format!(
                "session {} conversation link token is empty",
                id
            )))
        }
        (_, Some(_)) => {}
    }
    if let Some(opening) = state.conversation_opening.as_ref() {
        opening.to_core(&layer2_core::SessionId(id.to_owned()))?;
        if state.server_now.is_none() {
            return Err(StoreError::Malformed(format!(
                "session {} conversation opening has no coordination clock",
                id
            )));
        }
    }
    if next < generation {
        return Err(StoreError::Malformed(format!(
            "session {} runtime generation floor is behind current authority",
            id
        )));
    }
    if let Some(lease) = state.lease.as_ref() {
        let lease = lease.to_core(&layer2_core::SessionId(id.to_owned()))?;
        if lease.operation == SessionOperation::Interact {
            let lease_epoch = interaction_token_epoch(&lease.token)?;
            if lease_epoch == 0 || lease_epoch != interaction_epoch {
                return Err(StoreError::Malformed(format!(
                    "session {} interaction lease has an invalid epoch",
                    id
                )));
            }
        }
    }
    Ok(())
}

pub(super) fn has_session_lease(session: &SessionDoc) -> Result<bool, StoreError> {
    validate_coordination_state(session)?;
    let Some(state) = session.coordination.as_ref() else {
        return Ok(false);
    };
    let Some(lease) = state.lease.as_ref() else {
        return Ok(false);
    };
    let now = state.server_now.ok_or_else(|| {
        StoreError::Malformed("MongoDB coordination clock is absent during lease check".into())
    })?;
    Ok(lease.expires_at.timestamp_millis() > now.timestamp_millis())
}

fn current_boundary(session: &SessionDoc) -> Result<SessionBoundary, StoreError> {
    Ok(SessionBoundary {
        status: parse_plain(&session.status)?,
        authority: runtime_authority_from_doc(session)?,
    })
}

fn stored_generation(generation: u64) -> Result<i64, StoreError> {
    i64::try_from(generation)
        .map_err(|_| StoreError::Malformed("runtime generation cannot be stored".into()))
}

fn loaded_generation(generation: i64) -> Result<u64, StoreError> {
    u64::try_from(generation)
        .map_err(|_| StoreError::Malformed("runtime generation must not be negative".into()))
}

fn interaction_token_epoch(token: &str) -> Result<u64, StoreError> {
    token
        .strip_prefix("interaction:")
        .and_then(|epoch| epoch.parse().ok())
        .ok_or_else(|| StoreError::Malformed("invalid bounded interaction lease token".into()))
}

fn operation_name(operation: SessionOperation) -> &'static str {
    match operation {
        SessionOperation::Interact => "interact",
        SessionOperation::Provision => "provision",
        SessionOperation::Handover => "handover",
        SessionOperation::Recover => "recover",
        SessionOperation::Readopt => "readopt",
        SessionOperation::Reclaim => "reclaim",
        SessionOperation::Lifecycle => "lifecycle",
    }
}

fn parse_operation(operation: &str) -> Result<SessionOperation, StoreError> {
    match operation {
        "interact" => Ok(SessionOperation::Interact),
        "provision" => Ok(SessionOperation::Provision),
        "handover" => Ok(SessionOperation::Handover),
        "recover" => Ok(SessionOperation::Recover),
        "readopt" => Ok(SessionOperation::Readopt),
        "reclaim" => Ok(SessionOperation::Reclaim),
        "lifecycle" => Ok(SessionOperation::Lifecycle),
        other => Err(StoreError::Malformed(format!(
            "unknown stored session operation {other:?}"
        ))),
    }
}

fn cleanup_reason_name(reason: RuntimeCleanupReason) -> &'static str {
    match reason {
        RuntimeCleanupReason::UncommittedCandidate => "uncommitted_candidate",
        RuntimeCleanupReason::RetiredAuthority => "retired_authority",
    }
}

fn parse_cleanup_reason(reason: &str) -> Result<RuntimeCleanupReason, StoreError> {
    match reason {
        "uncommitted_candidate" => Ok(RuntimeCleanupReason::UncommittedCandidate),
        "retired_authority" => Ok(RuntimeCleanupReason::RetiredAuthority),
        other => Err(StoreError::Malformed(format!(
            "unknown stored runtime cleanup reason {other:?}"
        ))),
    }
}

fn candidate_cleanup_token(token: &str) -> String {
    format!("candidate:{token}")
}

fn retired_cleanup_token(token: &str) -> String {
    format!("retired:{token}")
}

fn duration_millis(duration: std::time::Duration) -> Result<i64, StoreError> {
    let millis = duration.as_millis();
    let millis = if duration.is_zero() { 0 } else { millis.max(1) };
    i64::try_from(millis)
        .map_err(|_| StoreError::Malformed("session lease duration cannot be stored".into()))
}

fn expiry(now: DateTime, ttl: std::time::Duration) -> Result<DateTime, StoreError> {
    now.timestamp_millis()
        .checked_add(duration_millis(ttl)?)
        .map(DateTime::from_millis)
        .ok_or_else(|| StoreError::Malformed("session lease expiry overflow".into()))
}

fn expire_lease(state: &mut CoordinationDoc, now: DateTime) {
    if state
        .lease
        .as_ref()
        .is_some_and(|lease| lease.expires_at.timestamp_millis() <= now.timestamp_millis())
    {
        state.lease = None;
    }
}

fn expire_conversation_opening(state: &mut CoordinationDoc, now: DateTime) {
    if state
        .conversation_opening
        .as_ref()
        .is_some_and(|opening| opening.expires_at.timestamp_millis() <= now.timestamp_millis())
    {
        state.conversation_opening = None;
    }
}

fn require_conversation_backing(
    session: &mut SessionDoc,
    opening: &ConversationOpeningLease,
    session_lease: Option<&SessionLease>,
    now: DateTime,
) -> Result<(), StoreError> {
    match opening.session_lease_token.as_deref() {
        Some(expected_token) => {
            let Some(lease) = session_lease else {
                return Err(StoreError::ConversationOpeningLost {
                    id: opening.session_id.clone(),
                });
            };
            if lease.session_id != opening.session_id
                || lease.holder != opening.holder
                || lease.token != expected_token
                || lease.operation == SessionOperation::Interact
                || require_current_lease(session, lease, now).is_err()
            {
                return Err(StoreError::ConversationOpeningLost {
                    id: opening.session_id.clone(),
                });
            }
            if opening.runtime_candidate_token.is_none()
                && (runtime_authority_from_doc(session)?.generation != opening.runtime_generation
                    || session.runtime_id.as_deref() != Some(opening.runtime_id.as_str()))
            {
                return Err(StoreError::ConversationOpeningLost {
                    id: opening.session_id.clone(),
                });
            }
        }
        None => {
            if opening.runtime_candidate_token.is_some()
                || session_lease.is_some()
                || opening.runtime_generation != 0
                || runtime_authority_from_doc(session)?.generation != 0
                || session.runtime_id.as_deref() != Some(opening.runtime_id.as_str())
            {
                return Err(StoreError::ConversationOpeningLost {
                    id: opening.session_id.clone(),
                });
            }
        }
    }
    Ok(())
}

pub(super) fn require_conversation_write_authority(
    session: &mut SessionDoc,
    authority: &ConversationWriteAuthority,
) -> Result<(), StoreError> {
    validate_coordination_state(session)?;
    let id = layer2_core::SessionId(session.id.clone());
    let now = session
        .coordination
        .as_ref()
        .and_then(|state| state.server_now)
        .ok_or_else(|| {
            StoreError::Malformed(
                "MongoDB coordination clock is absent during conversation write".into(),
            )
        })?;
    match authority {
        ConversationWriteAuthority::Opening {
            opening,
            session_lease,
        } => {
            if opening.session_id != id {
                return Err(StoreError::ConversationOpeningLost { id });
            }
            {
                let state = session
                    .coordination
                    .get_or_insert_with(CoordinationDoc::default);
                expire_lease(state, now);
                expire_conversation_opening(state, now);
                let matches = state
                    .conversation_opening
                    .as_ref()
                    .map_or(Ok(false), |stored| stored.matches(opening))?;
                if !matches {
                    return Err(StoreError::ConversationOpeningLost { id });
                }
            }
            require_conversation_backing(session, opening, session_lease.as_ref(), now)
        }
        ConversationWriteAuthority::Active(link) => {
            let state = session
                .coordination
                .as_ref()
                .ok_or_else(|| StoreError::ConversationLinkChanged { id: id.clone() })?;
            if link.session_id != id
                || link.epoch == 0
                || loaded_generation(state.conversation_epoch)? != link.epoch
                || state.conversation_link_token.as_deref() != Some(link.token.as_str())
                || loaded_generation(state.authority_generation)? != link.runtime_generation
                || session.runtime_id.as_deref() != Some(link.runtime_id.as_str())
            {
                return Err(StoreError::ConversationLinkChanged { id });
            }
            Ok(())
        }
    }
}

fn receipt<'a>(
    receipts: &'a [CoordinationReceiptDoc],
    expected: &CoordinationReceiptIdDoc,
) -> Result<Option<&'a CoordinationReceiptDoc>, StoreError> {
    let found = receipts.iter().find(|receipt| receipt.id == *expected);
    if let Some(found) = found {
        found.validate_id(expected)?;
    }
    Ok(found)
}

fn require_registered_conversation_candidate(
    receipts: &[CoordinationReceiptDoc],
    opening: &ConversationOpeningLease,
    session_lease: Option<&SessionLease>,
) -> Result<(), StoreError> {
    let Some(token) = opening.runtime_candidate_token.as_ref() else {
        return Ok(());
    };
    let Some(lease) = session_lease else {
        return Err(StoreError::ConversationOpeningLost {
            id: opening.session_id.clone(),
        });
    };
    let receipt_id = CoordinationReceiptIdDoc::new(&opening.session_id, RECEIPT_RESERVATION, token);
    let Some(receipt) = receipt(receipts, &receipt_id)? else {
        return Err(StoreError::ConversationOpeningLost {
            id: opening.session_id.clone(),
        });
    };
    let CoordinationReceiptPayloadDoc::Reservation(candidate) = &receipt.payload else {
        return Err(StoreError::Malformed(
            "MongoDB conversation candidate receipt carries another payload kind".into(),
        ));
    };
    if opening.runtime_generation == 0
        || candidate.lease_token != lease.token
        || loaded_generation(candidate.generation)? != opening.runtime_generation
        || candidate.runtime_id.as_deref() != Some(opening.runtime_id.as_str())
    {
        return Err(StoreError::ConversationOpeningLost {
            id: opening.session_id.clone(),
        });
    }
    Ok(())
}

fn require_current_lease(
    session: &mut SessionDoc,
    lease: &SessionLease,
    now: DateTime,
) -> Result<(), StoreError> {
    let current = current_boundary(session)?;
    require_current_lease_identity(session, lease, now)?;
    if current != lease.boundary {
        return Err(StoreError::SessionLeaseLost {
            id: lease.session_id.clone(),
        });
    }
    Ok(())
}

fn require_current_lease_identity(
    session: &mut SessionDoc,
    lease: &SessionLease,
    now: DateTime,
) -> Result<(), StoreError> {
    let state = session
        .coordination
        .get_or_insert_with(CoordinationDoc::default);
    expire_lease(state, now);
    let Some(stored) = state.lease.as_ref() else {
        return Err(StoreError::SessionLeaseLost {
            id: lease.session_id.clone(),
        });
    };
    if !stored.matches_lease(lease)? {
        return Err(StoreError::SessionLeaseLost {
            id: lease.session_id.clone(),
        });
    }
    Ok(())
}

fn require_current_interaction_lease(
    session: &mut SessionDoc,
    lease: &SessionLease,
    now: DateTime,
) -> Result<(), StoreError> {
    if lease.operation != SessionOperation::Interact {
        return Err(StoreError::Malformed(
            "provider-operation leases require provider renewal".into(),
        ));
    }
    let current = current_boundary(session)?;
    require_current_lease_identity(session, lease, now)?;
    if current.authority != lease.boundary.authority {
        return Err(StoreError::SessionLeaseLost {
            id: lease.session_id.clone(),
        });
    }
    Ok(())
}

fn next_revision(session: &mut SessionDoc) -> Result<u64, StoreError> {
    let actual = u64::try_from(session.revision)
        .map_err(|_| StoreError::Malformed("session revision must not be negative".into()))?;
    let next = actual
        .checked_add(1)
        .ok_or_else(|| StoreError::Malformed("session revision overflow".into()))?;
    session.revision = i64::try_from(next)
        .map_err(|_| StoreError::Malformed("session revision cannot be stored".into()))?;
    Ok(next)
}

fn next_runtime_started_sequence(sequence_floor: u64) -> Result<(u64, i64), StoreError> {
    let next = sequence_floor
        .checked_add(1)
        .ok_or_else(|| StoreError::Malformed("session event sequence overflow".into()))?;
    let stored = i64::try_from(next)
        .map_err(|_| StoreError::Malformed("session event sequence cannot be stored".into()))?;
    Ok((next, stored))
}

fn cleanup_receipt(
    id: &layer2_core::SessionId,
    cleanup: &RuntimeCleanup,
) -> Result<CoordinationReceiptDoc, StoreError> {
    Ok(CoordinationReceiptDoc::new(
        CoordinationReceiptIdDoc::new(id, RECEIPT_CLEANUP, &cleanup.token),
        CoordinationReceiptPayloadDoc::Cleanup(CleanupReceiptDoc {
            cleanup: CleanupDoc::from_core(cleanup)?,
            completed_by: None,
        }),
    ))
}

fn cleanup_from_receipt(receipt: &CoordinationReceiptDoc) -> Result<RuntimeCleanup, StoreError> {
    receipt.validate_stored()?;
    let CoordinationReceiptPayloadDoc::Cleanup(stored) = &receipt.payload else {
        return Err(StoreError::Malformed(
            "MongoDB cleanup receipt carries another payload kind".into(),
        ));
    };
    stored.cleanup.to_core(
        &layer2_core::SessionId(receipt.id.session_id.clone()),
        receipt.id.token.clone(),
    )
}

fn require_lease_receipt<'a>(
    receipts: &'a BTreeMap<CoordinationReceiptIdDoc, CoordinationReceiptDoc>,
    id: &layer2_core::SessionId,
    token: &str,
) -> Result<&'a LeaseTokenReceiptDoc, StoreError> {
    let receipt_id = CoordinationReceiptIdDoc::new(id, RECEIPT_LEASE, token);
    let receipt = receipts.get(&receipt_id).ok_or_else(|| {
        StoreError::Malformed(format!(
            "MongoDB coordination state for {id} is missing lease receipt {token:?}"
        ))
    })?;
    let CoordinationReceiptPayloadDoc::LeaseToken(stored) = &receipt.payload else {
        return Err(StoreError::Malformed(format!(
            "MongoDB coordination state for {id} has a non-lease receipt for lease {token:?}"
        )));
    };
    Ok(stored)
}

fn require_cleanup_receipt(
    receipts: &BTreeMap<CoordinationReceiptIdDoc, CoordinationReceiptDoc>,
    referenced_cleanups: &mut BTreeSet<CoordinationReceiptIdDoc>,
    cleanup: &RuntimeCleanup,
) -> Result<(), StoreError> {
    let receipt_id =
        CoordinationReceiptIdDoc::new(&cleanup.session_id, RECEIPT_CLEANUP, &cleanup.token);
    let receipt = receipts.get(&receipt_id).ok_or_else(|| {
        StoreError::Malformed(format!(
            "MongoDB coordination state for {} is missing cleanup receipt {:?}",
            cleanup.session_id, cleanup.token
        ))
    })?;
    if cleanup_from_receipt(receipt)? != *cleanup {
        return Err(StoreError::Malformed(format!(
            "MongoDB cleanup receipt {:?} disagrees with its originating operation",
            cleanup.token
        )));
    }
    if !referenced_cleanups.insert(receipt_id) {
        return Err(StoreError::Malformed(format!(
            "MongoDB cleanup receipt {:?} has more than one originating operation",
            cleanup.token
        )));
    }
    Ok(())
}

fn coordination_receipt_from_document(
    document: Document,
) -> Result<CoordinationReceiptDoc, StoreError> {
    let payload = document.get_document("payload").map_err(|_| {
        StoreError::Malformed("MongoDB coordination receipt payload must be a document".into())
    })?;
    let kind = payload.get_str("type").map_err(|_| {
        StoreError::Malformed("MongoDB coordination receipt payload type must be a string".into())
    })?;
    let value = payload.get_document("value").map_err(|_| {
        StoreError::Malformed(
            "MongoDB coordination receipt payload value must be a document".into(),
        )
    })?;
    match (kind, value.get("completed_by")) {
        (RECEIPT_CLEANUP, None) => {}
        (RECEIPT_CLEANUP, Some(Bson::String(completed_by))) if !completed_by.is_empty() => {}
        (RECEIPT_CLEANUP, Some(_)) => {
            return Err(StoreError::Malformed(
                "MongoDB cleanup receipt completion must be a non-empty string when present".into(),
            ));
        }
        (_, None) => {}
        (_, Some(_)) => {
            return Err(StoreError::Malformed(
                "MongoDB non-cleanup receipt carries cleanup completion metadata".into(),
            ));
        }
    }

    let receipt: CoordinationReceiptDoc = mongodb::bson::deserialize_from_document(document)
        .map_err(|error| {
            StoreError::Malformed(format!("decode MongoDB coordination receipt: {error}"))
        })?;
    receipt.validate_stored()?;
    Ok(receipt)
}

fn coordination_receipt_map(
    documents: impl IntoIterator<Item = Document>,
) -> Result<BTreeMap<CoordinationReceiptIdDoc, CoordinationReceiptDoc>, StoreError> {
    let mut receipts = BTreeMap::new();
    for document in documents {
        let receipt = coordination_receipt_from_document(document)?;
        if receipts.insert(receipt.id.clone(), receipt).is_some() {
            return Err(StoreError::Malformed(
                "MongoDB coordination receipts contain a duplicate logical identity".into(),
            ));
        }
    }
    Ok(receipts)
}

fn runtime_started_marker_expectations(
    receipts: &BTreeMap<CoordinationReceiptIdDoc, CoordinationReceiptDoc>,
) -> Result<BTreeMap<(String, u64), NonZeroU64>, StoreError> {
    let mut expected = BTreeMap::new();
    for receipt in receipts.values() {
        let CoordinationReceiptPayloadDoc::Reservation(reservation) = &receipt.payload else {
            continue;
        };
        let Some(completed) = reservation.switch.as_ref() else {
            continue;
        };
        let seq = NonZeroU64::new(loaded_generation(completed.runtime_started_seq)?)
            .ok_or_else(|| {
                StoreError::Malformed("MongoDB completed switch has no runtime-start marker".into())
            })?
            .get();
        let generation =
            NonZeroU64::new(loaded_generation(reservation.generation)?).ok_or_else(|| {
                StoreError::Malformed("MongoDB reservation receipt generation is zero".into())
            })?;
        let key = (receipt.id.session_id.clone(), seq);
        if expected.insert(key, generation).is_some() {
            return Err(StoreError::Malformed(
                "MongoDB completed switches name a duplicate runtime-start marker position".into(),
            ));
        }
    }
    Ok(expected)
}

fn runtime_started_marker_filter(session_id: &str, seq: u64) -> Result<Document, StoreError> {
    let seq = i64::try_from(seq).map_err(|_| {
        StoreError::Malformed("runtime-start marker sequence cannot be stored".into())
    })?;
    Ok(doc! { "session_id": session_id, "seq": seq })
}

#[derive(Debug, Clone, PartialEq)]
struct RuntimeStartedMarkerAuditBatch {
    filter: Document,
    expectations: BTreeMap<(String, u64), NonZeroU64>,
}

fn runtime_started_marker_audit_batches(
    expectations: &BTreeMap<(String, u64), NonZeroU64>,
) -> Result<Vec<RuntimeStartedMarkerAuditBatch>, StoreError> {
    let mut batches = Vec::new();
    let mut clauses = Vec::with_capacity(RUNTIME_STARTED_MARKER_AUDIT_BATCH_SIZE);
    let mut batch_expectations = BTreeMap::new();
    for ((session_id, seq), generation) in expectations {
        clauses.push(runtime_started_marker_filter(session_id, *seq)?);
        batch_expectations.insert((session_id.clone(), *seq), *generation);
        if clauses.len() == RUNTIME_STARTED_MARKER_AUDIT_BATCH_SIZE {
            batches.push(RuntimeStartedMarkerAuditBatch {
                filter: doc! { "$or": std::mem::take(&mut clauses) },
                expectations: std::mem::take(&mut batch_expectations),
            });
        }
    }
    if !clauses.is_empty() {
        batches.push(RuntimeStartedMarkerAuditBatch {
            filter: doc! { "$or": clauses },
            expectations: batch_expectations,
        });
    }
    Ok(batches)
}

fn runtime_started_marker_position(document: &Document) -> Result<(String, u64), StoreError> {
    let session_id = document.get_str("session_id").map_err(|_| {
        StoreError::Malformed("MongoDB runtime-start marker session_id must be a string".into())
    })?;
    let seq = document.get_i64("seq").map_err(|_| {
        StoreError::Malformed("MongoDB runtime-start marker seq must be a 64-bit integer".into())
    })?;
    let seq = u64::try_from(seq).map_err(|_| {
        StoreError::Malformed("MongoDB runtime-start marker seq must not be negative".into())
    })?;
    Ok((session_id.to_owned(), seq))
}

fn validate_runtime_started_marker(
    session_id: &str,
    seq: u64,
    expected_generation: NonZeroU64,
    document: Option<Document>,
) -> Result<(), StoreError> {
    let document = document.ok_or_else(|| {
        StoreError::Malformed(format!(
            "MongoDB completed switch is missing runtime-start marker {session_id}:{seq}"
        ))
    })?;
    let event = super::event_doc::from_document(document)?;
    if event.session_id.0 != session_id || event.seq != seq {
        return Err(StoreError::Malformed(format!(
            "MongoDB runtime-start marker query returned another event position for {session_id}:{seq}"
        )));
    }
    match event.kind {
        StoredEventKind::Known(EventKind::RuntimeStarted { generation })
            if generation == expected_generation =>
        {
            Ok(())
        }
        StoredEventKind::Known(EventKind::RuntimeStarted { .. }) => {
            Err(StoreError::Malformed(format!(
                "MongoDB completed switch disagrees with runtime-start marker {session_id}:{seq}"
            )))
        }
        _ => Err(StoreError::Malformed(format!(
            "MongoDB completed switch position {session_id}:{seq} is not a runtime-start marker"
        ))),
    }
}

fn validate_runtime_started_marker_batch(
    batch: &RuntimeStartedMarkerAuditBatch,
    documents: Vec<Document>,
) -> Result<(), StoreError> {
    let mut found = BTreeSet::new();
    for document in documents {
        let position = runtime_started_marker_position(&document)?;
        let Some(expected_generation) = batch.expectations.get(&position).copied() else {
            return Err(StoreError::Malformed(format!(
                "MongoDB runtime-start marker batch returned unexpected event position {}:{}",
                position.0, position.1
            )));
        };
        if !found.insert(position.clone()) {
            return Err(StoreError::Malformed(format!(
                "MongoDB runtime-start marker batch returned duplicate event position {}:{}",
                position.0, position.1
            )));
        }
        validate_runtime_started_marker(
            &position.0,
            position.1,
            expected_generation,
            Some(document),
        )?;
    }
    if let Some(((session_id, seq), expected_generation)) = batch
        .expectations
        .iter()
        .find(|(position, _)| !found.contains(*position))
    {
        return validate_runtime_started_marker(session_id, *seq, *expected_generation, None);
    }
    Ok(())
}

pub(super) fn validate_coordination_receipt_set(
    sessions: &BTreeMap<String, SessionDoc>,
    receipts: &BTreeMap<CoordinationReceiptIdDoc, CoordinationReceiptDoc>,
) -> Result<(), StoreError> {
    for session in sessions.values() {
        validate_coordination_state(session)?;
        let expected_receipts = receipts
            .keys()
            .filter(|receipt| receipt.session_id == session.id)
            .count();
        let stored_receipts = session
            .coordination
            .as_ref()
            .map_or(Ok(0), |state| loaded_generation(state.receipt_count))?;
        if stored_receipts != u64::try_from(expected_receipts).unwrap_or(u64::MAX) {
            return Err(StoreError::Malformed(format!(
                "MongoDB session {} records {stored_receipts} coordination receipts but stores {expected_receipts}",
                session.id
            )));
        }
        if let Some(lease) = session
            .coordination
            .as_ref()
            .and_then(|state| state.lease.as_ref())
        {
            let id = layer2_core::SessionId(session.id.clone());
            if parse_operation(&lease.operation)? != SessionOperation::Interact {
                let stored = require_lease_receipt(receipts, &id, &lease.token)?;
                if stored.released.is_some() {
                    return Err(StoreError::Malformed(format!(
                        "MongoDB session {id} has an active lease whose receipt is released"
                    )));
                }
            }
        }
    }

    let mut referenced_cleanups = BTreeSet::new();
    let mut referenced_runtime_started = BTreeSet::new();
    for receipt in receipts.values() {
        receipt.validate_stored()?;
        let session = sessions.get(&receipt.id.session_id).ok_or_else(|| {
            StoreError::Malformed(format!(
                "MongoDB coordination receipts for {} have no session owner",
                receipt.id.session_id
            ))
        })?;
        let id = layer2_core::SessionId(receipt.id.session_id.clone());
        let generation_floor = session
            .coordination
            .as_ref()
            .map_or(Ok(0), |state| loaded_generation(state.next_generation))?;
        match &receipt.payload {
            CoordinationReceiptPayloadDoc::LeaseToken(_) => {}
            CoordinationReceiptPayloadDoc::Reservation(stored) => {
                require_lease_receipt(receipts, &id, &stored.lease_token)?;
                let generation = loaded_generation(stored.generation)?;
                if generation > generation_floor {
                    return Err(StoreError::Malformed(format!(
                        "MongoDB reservation {:?} is ahead of session {id}'s generation floor",
                        receipt.id.token
                    )));
                }
                if let Some(runtime_id) = stored.runtime_id.as_ref() {
                    let candidate_cleanup = RuntimeCleanup {
                        session_id: id.clone(),
                        generation,
                        runtime_id: runtime_id.clone(),
                        token: candidate_cleanup_token(&receipt.id.token),
                        reason: RuntimeCleanupReason::UncommittedCandidate,
                    };
                    if let Some(completed) = stored.switch.as_ref() {
                        let runtime_started_seq = loaded_generation(completed.runtime_started_seq)?;
                        if runtime_started_seq != 0 {
                            let session_tail = u64::try_from(session.last_seq).map_err(|_| {
                                StoreError::Malformed(format!(
                                    "MongoDB session {id} event tail must not be negative"
                                ))
                            })?;
                            if runtime_started_seq > session_tail {
                                return Err(StoreError::Malformed(format!(
                                    "MongoDB completed switch {:?} names runtime-start marker {runtime_started_seq} beyond session {id}'s event tail {session_tail}",
                                    receipt.id.token
                                )));
                            }
                            if !referenced_runtime_started
                                .insert((session.id.clone(), runtime_started_seq))
                            {
                                return Err(StoreError::Malformed(format!(
                                    "MongoDB completed switches for session {id} name the same runtime-start marker {runtime_started_seq}"
                                )));
                            }
                        }
                        let candidate_id = CoordinationReceiptIdDoc::new(
                            &id,
                            RECEIPT_CLEANUP,
                            &candidate_cleanup.token,
                        );
                        if receipts.contains_key(&candidate_id) {
                            return Err(StoreError::Malformed(format!(
                                "MongoDB completed switch {:?} retains its candidate cleanup",
                                receipt.id.token
                            )));
                        }
                        if let Some(cleanup) = completed.cleanup.as_ref() {
                            require_cleanup_receipt(
                                receipts,
                                &mut referenced_cleanups,
                                &cleanup.to_core(&id, retired_cleanup_token(&receipt.id.token))?,
                            )?;
                        }
                    } else {
                        require_cleanup_receipt(
                            receipts,
                            &mut referenced_cleanups,
                            &candidate_cleanup,
                        )?;
                    }
                }
            }
            CoordinationReceiptPayloadDoc::Retirement(stored) => {
                require_lease_receipt(receipts, &id, &stored.lease_token)?;
                require_cleanup_receipt(
                    receipts,
                    &mut referenced_cleanups,
                    &stored
                        .outcome
                        .cleanup
                        .to_core(&id, retired_cleanup_token(&receipt.id.token))?,
                )?;
            }
            CoordinationReceiptPayloadDoc::Cleanup(stored) => {
                if loaded_generation(stored.cleanup.generation)? > generation_floor {
                    return Err(StoreError::Malformed(format!(
                        "MongoDB cleanup {:?} is ahead of session {id}'s generation floor",
                        receipt.id.token
                    )));
                }
                if let Some(completed_by) = stored.completed_by.as_ref() {
                    require_lease_receipt(receipts, &id, completed_by)?;
                }
            }
        }
    }
    for receipt in receipts.values() {
        if matches!(&receipt.payload, CoordinationReceiptPayloadDoc::Cleanup(_))
            && !referenced_cleanups.contains(&receipt.id)
        {
            return Err(StoreError::Malformed(format!(
                "MongoDB cleanup receipt {:?} has no originating operation",
                receipt.id.token
            )));
        }
    }
    Ok(())
}

pub(super) fn validate_session_coordination_receipts(
    session: Option<&SessionDoc>,
    documents: Vec<Document>,
) -> Result<usize, StoreError> {
    let receipts = coordination_receipt_map(documents)?;
    let sessions = session
        .map(|session| BTreeMap::from([(session.id.clone(), session.clone())]))
        .unwrap_or_default();
    validate_coordination_receipt_set(&sessions, &receipts)?;
    Ok(receipts
        .values()
        .filter(|receipt| {
            matches!(
                &receipt.payload,
                CoordinationReceiptPayloadDoc::Cleanup(stored) if stored.completed_by.is_none()
            )
        })
        .count())
}

pub(super) fn coordination_clock_update() -> Document {
    doc! {
        "$set": {
            super::WRITE_PROTOCOL_FIELD: super::SESSION_WRITE_PROTOCOL,
            super::COORDINATION_PROTOCOL_FIELD: super::SESSION_COORDINATION_PROTOCOL,
        },
        "$max": { "revision": 0_i64 },
        "$currentDate": { "coordination.server_now": true },
    }
}

#[cfg(test)]
fn apply_command(
    session: &mut SessionDoc,
    command: &CoordinationCommand,
    receipts: &[CoordinationReceiptDoc],
) -> Result<(CoordinationOutcome, Vec<ReceiptMutation>, Option<Event>), StoreError> {
    let sequence_floor = u64::try_from(session.last_seq)
        .map_err(|_| StoreError::Malformed("session event tail must not be negative".into()))?;
    apply_command_at_sequence_floor(session, command, receipts, sequence_floor)
}

fn apply_command_at_sequence_floor(
    session: &mut SessionDoc,
    command: &CoordinationCommand,
    receipts: &[CoordinationReceiptDoc],
    sequence_floor: u64,
) -> Result<(CoordinationOutcome, Vec<ReceiptMutation>, Option<Event>), StoreError> {
    let previous_last_seq = session.last_seq;
    let recorded_tail = u64::try_from(previous_last_seq)
        .map_err(|_| StoreError::Malformed("session event tail must not be negative".into()))?;
    if sequence_floor < recorded_tail {
        return Err(StoreError::Malformed(format!(
            "session {} event sequence floor {sequence_floor} is behind recorded tail {recorded_tail}",
            session.id
        )));
    }
    let server_now = session
        .coordination
        .as_ref()
        .and_then(|state| state.server_now)
        .ok_or_else(|| StoreError::Malformed("MongoDB coordination clock is absent".into()))?;
    let (outcome, mutations) =
        apply_command_transition(session, command, receipts, sequence_floor)?;
    let inserted = mutations
        .iter()
        .filter(|mutation| matches!(mutation, ReceiptMutation::Insert(_)))
        .count();
    let deleted = mutations
        .iter()
        .filter(|mutation| matches!(mutation, ReceiptMutation::Delete(_)))
        .count();
    if inserted != 0 || deleted != 0 {
        let state = session
            .coordination
            .get_or_insert_with(CoordinationDoc::default);
        let current = loaded_generation(state.receipt_count)?;
        let inserted = u64::try_from(inserted).map_err(|_| {
            StoreError::Malformed("MongoDB coordination receipt count cannot be stored".into())
        })?;
        let deleted = u64::try_from(deleted).map_err(|_| {
            StoreError::Malformed("MongoDB coordination receipt count cannot be stored".into())
        })?;
        let current = current
            .checked_add(inserted)
            .and_then(|count| count.checked_sub(deleted))
            .ok_or_else(|| {
                StoreError::Malformed("MongoDB coordination receipt count overflow".into())
            })?;
        state.receipt_count = stored_generation(current)?;
    }
    let runtime_started = if session.last_seq == previous_last_seq {
        None
    } else {
        let (CoordinationCommand::Switch { candidate, .. }, CoordinationOutcome::Switch(switched)) =
            (command, &outcome)
        else {
            return Err(StoreError::Malformed(
                "a non-switch coordination command changed the event tail".into(),
            ));
        };
        let expected = sequence_floor
            .checked_add(1)
            .ok_or_else(|| StoreError::Malformed("session event sequence overflow".into()))?;
        let expected_stored = i64::try_from(expected)
            .map_err(|_| StoreError::Malformed("session event sequence cannot be stored".into()))?;
        if session.last_seq != expected_stored || switched.runtime_started_seq != expected {
            return Err(StoreError::Malformed(
                "runtime authority switch did not allocate the next event sequence".into(),
            ));
        }
        Some(Event {
            session_id: candidate.session_id.clone(),
            seq: expected,
            kind: EventKind::RuntimeStarted {
                generation: candidate.generation,
            },
            appended_at: layer2_core::rfc3339_at(server_now.to_system_time()),
        })
    };
    Ok((outcome, mutations, runtime_started))
}

fn apply_command_transition(
    session: &mut SessionDoc,
    command: &CoordinationCommand,
    receipts: &[CoordinationReceiptDoc],
    runtime_started_sequence_floor: u64,
) -> Result<(CoordinationOutcome, Vec<ReceiptMutation>), StoreError> {
    if session.id != command.id().0 {
        return Err(StoreError::SessionDeleted {
            id: command.id().clone(),
        });
    }
    validate_coordination_state(session)?;
    let now = session
        .coordination
        .as_ref()
        .and_then(|state| state.server_now)
        .ok_or_else(|| StoreError::Malformed("MongoDB coordination clock is absent".into()))?;

    match command {
        CoordinationCommand::Acquire(request) => {
            if request.operation == SessionOperation::Interact {
                return Err(StoreError::Malformed(
                    "interaction leases require bounded interaction acquisition".into(),
                ));
            }
            let current = current_boundary(session)?;
            if current != request.boundary {
                return Err(StoreError::SessionAuthorityChanged {
                    id: request.session_id.clone(),
                });
            }
            let receipt_id =
                CoordinationReceiptIdDoc::new(&request.session_id, RECEIPT_LEASE, &request.token);
            let used = receipt(receipts, &receipt_id)?;
            let state = session
                .coordination
                .get_or_insert_with(CoordinationDoc::default);
            expire_lease(state, now);
            let expires_at = expiry(now, request.ttl)?;
            if let Some(used) = used {
                let CoordinationReceiptPayloadDoc::LeaseToken(stored) = &used.payload else {
                    return Err(StoreError::Malformed(
                        "MongoDB lease receipt carries another payload kind".into(),
                    ));
                };
                if stored.released.is_some() {
                    return Err(StoreError::SessionLeaseLost {
                        id: request.session_id.clone(),
                    });
                }
                let Some(active) = state.lease.as_mut() else {
                    return Err(StoreError::SessionLeaseLost {
                        id: request.session_id.clone(),
                    });
                };
                if !active.matches_request(request)? {
                    return Err(StoreError::SessionLeaseLost {
                        id: request.session_id.clone(),
                    });
                }
                active.expires_at = expires_at;
                return Ok((
                    CoordinationOutcome::Lease(active.to_core(&request.session_id)?),
                    Vec::new(),
                ));
            }
            if state.lease.is_some() {
                return Err(StoreError::SessionLeaseHeld {
                    id: request.session_id.clone(),
                });
            }
            let lease = LeaseDoc::from_request(request, expires_at)?;
            let outcome = lease.to_core(&request.session_id)?;
            state.lease = Some(lease);
            Ok((
                CoordinationOutcome::Lease(outcome),
                vec![ReceiptMutation::Insert(CoordinationReceiptDoc::new(
                    receipt_id,
                    CoordinationReceiptPayloadDoc::LeaseToken(LeaseTokenReceiptDoc::default()),
                ))],
            ))
        }
        CoordinationCommand::AcquireInteraction(request) => {
            if request.holder.is_empty() || request.nonce.is_empty() {
                return Err(StoreError::Malformed(
                    "session interaction lease holder and nonce must not be empty".into(),
                ));
            }
            let current = current_boundary(session)?;
            if current != request.boundary {
                return Err(StoreError::SessionAuthorityChanged {
                    id: request.session_id.clone(),
                });
            }
            let state = session
                .coordination
                .get_or_insert_with(CoordinationDoc::default);
            expire_lease(state, now);
            let expires_at = expiry(now, request.ttl)?;
            if let Some(held) = state.lease.as_mut() {
                if held.matches_interaction_request(request)? {
                    held.expires_at = expires_at;
                    return Ok((
                        CoordinationOutcome::Lease(held.to_core(&request.session_id)?),
                        Vec::new(),
                    ));
                }
                return Err(StoreError::SessionInteractionBlocked {
                    id: request.session_id.clone(),
                    operation: parse_operation(&held.operation)?,
                });
            }
            let epoch = loaded_generation(state.interaction_epoch)?
                .checked_add(1)
                .ok_or_else(|| StoreError::Malformed("interaction lease epoch overflow".into()))?;
            state.interaction_epoch = stored_generation(epoch)?;
            let lease =
                LeaseDoc::from_interaction(request, format!("interaction:{epoch}"), expires_at)?;
            let outcome = lease.to_core(&request.session_id)?;
            state.lease = Some(lease);
            Ok((CoordinationOutcome::Lease(outcome), Vec::new()))
        }
        CoordinationCommand::Renew { lease, ttl } => {
            if lease.operation == SessionOperation::Interact {
                return Err(StoreError::Malformed(
                    "interaction leases require interaction renewal".into(),
                ));
            }
            require_current_lease(session, lease, now)?;
            session
                .coordination
                .as_mut()
                .and_then(|state| state.lease.as_mut())
                .expect("the current lease was validated")
                .expires_at = expiry(now, *ttl)?;
            Ok((CoordinationOutcome::Unit, Vec::new()))
        }
        CoordinationCommand::RenewInteraction { lease, ttl } => {
            require_current_interaction_lease(session, lease, now)?;
            session
                .coordination
                .as_mut()
                .and_then(|state| state.lease.as_mut())
                .expect("the current interaction lease was validated")
                .expires_at = expiry(now, *ttl)?;
            Ok((CoordinationOutcome::Unit, Vec::new()))
        }
        CoordinationCommand::Advance { lease, boundary } => {
            let current = current_boundary(session)?;
            if current != *boundary {
                return Err(StoreError::SessionLeaseLost {
                    id: lease.session_id.clone(),
                });
            }
            let state = session
                .coordination
                .get_or_insert_with(CoordinationDoc::default);
            expire_lease(state, now);
            let Some(active) = state.lease.as_mut() else {
                return Err(StoreError::SessionLeaseLost {
                    id: lease.session_id.clone(),
                });
            };
            let already_advanced = active.same_identity(lease)?
                && active.boundary == BoundaryDoc::from_core(boundary)?;
            if !active.matches_lease(lease)? && !already_advanced {
                return Err(StoreError::SessionLeaseLost {
                    id: lease.session_id.clone(),
                });
            }
            active.boundary = BoundaryDoc::from_core(boundary)?;
            Ok((CoordinationOutcome::Unit, Vec::new()))
        }
        CoordinationCommand::Release(lease) => {
            if lease.operation == SessionOperation::Interact {
                let lease_epoch = interaction_token_epoch(&lease.token).map_err(|_| {
                    StoreError::SessionLeaseLost {
                        id: lease.session_id.clone(),
                    }
                })?;
                let state = session
                    .coordination
                    .get_or_insert_with(CoordinationDoc::default);
                expire_lease(state, now);
                let current_epoch = loaded_generation(state.interaction_epoch)?;
                if lease_epoch == 0 || lease_epoch > current_epoch {
                    return Err(StoreError::SessionLeaseLost {
                        id: lease.session_id.clone(),
                    });
                }
                if let Some(active) = state.lease.as_ref() {
                    if parse_operation(&active.operation)? == SessionOperation::Interact
                        && lease_epoch == current_epoch
                    {
                        if !active.matches_lease(lease)? {
                            return Err(StoreError::SessionLeaseLost {
                                id: lease.session_id.clone(),
                            });
                        }
                        state.lease = None;
                    }
                }
                return Ok((CoordinationOutcome::Unit, Vec::new()));
            }
            let receipt_id =
                CoordinationReceiptIdDoc::new(&lease.session_id, RECEIPT_LEASE, &lease.token);
            let existing = receipt(receipts, &receipt_id)?.ok_or_else(|| {
                StoreError::Malformed("MongoDB current lease has no token receipt".into())
            })?;
            let CoordinationReceiptPayloadDoc::LeaseToken(mut stored) = existing.payload.clone()
            else {
                return Err(StoreError::Malformed(
                    "MongoDB lease receipt carries another payload kind".into(),
                ));
            };
            if let Some(released) = stored.released.as_ref() {
                return if released.matches_lease(lease)? {
                    Ok((CoordinationOutcome::Unit, Vec::new()))
                } else {
                    Err(StoreError::SessionLeaseLost {
                        id: lease.session_id.clone(),
                    })
                };
            }
            require_current_lease(session, lease, now)?;
            session
                .coordination
                .as_mut()
                .expect("the current lease was validated")
                .lease = None;
            stored.released = Some(LeaseReleaseDoc::from_core(lease)?);
            Ok((
                CoordinationOutcome::Unit,
                vec![ReceiptMutation::Replace(CoordinationReceiptDoc::new(
                    receipt_id,
                    CoordinationReceiptPayloadDoc::LeaseToken(stored),
                ))],
            ))
        }
        CoordinationCommand::AcquireConversationOpening {
            request,
            session_lease,
        } => {
            let expires_at = expiry(now, request.ttl)?;
            let opening =
                ConversationOpeningDoc::from_request(request, session_lease.as_ref(), expires_at)?;
            let outcome = opening.to_core(&request.session_id)?;
            require_conversation_backing(session, &outcome, session_lease.as_ref(), now)?;
            require_registered_conversation_candidate(receipts, &outcome, session_lease.as_ref())?;
            let state = session
                .coordination
                .get_or_insert_with(CoordinationDoc::default);
            expire_conversation_opening(state, now);
            if let Some(held) = state.conversation_opening.as_mut() {
                if !held.matches(&outcome)? {
                    return Err(StoreError::ConversationOpeningHeld {
                        id: request.session_id.clone(),
                    });
                }
                held.expires_at = expires_at;
                return Ok((
                    CoordinationOutcome::ConversationOpening(outcome),
                    Vec::new(),
                ));
            }
            state.conversation_opening = Some(opening);
            Ok((
                CoordinationOutcome::ConversationOpening(outcome),
                Vec::new(),
            ))
        }
        CoordinationCommand::RenewConversationOpening {
            opening,
            session_lease,
            ttl,
        } => {
            {
                let state = session
                    .coordination
                    .get_or_insert_with(CoordinationDoc::default);
                expire_conversation_opening(state, now);
                let matches = state
                    .conversation_opening
                    .as_ref()
                    .map_or(Ok(false), |held| held.matches(opening))?;
                if !matches {
                    return Err(StoreError::ConversationOpeningLost {
                        id: opening.session_id.clone(),
                    });
                }
            }
            require_conversation_backing(session, opening, session_lease.as_ref(), now)?;
            session
                .coordination
                .as_mut()
                .and_then(|state| state.conversation_opening.as_mut())
                .expect("the exact conversation opening was validated")
                .expires_at = expiry(now, *ttl)?;
            Ok((CoordinationOutcome::Unit, Vec::new()))
        }
        CoordinationCommand::PublishConversationLink {
            opening,
            session_lease,
        } => {
            let current_generation = runtime_authority_from_doc(session)?.generation;
            {
                let state = session
                    .coordination
                    .get_or_insert_with(CoordinationDoc::default);
                expire_conversation_opening(state, now);
                if state.conversation_link_token.as_deref() == Some(opening.token.as_str()) {
                    if current_generation != opening.runtime_generation
                        || session.runtime_id.as_deref() != Some(opening.runtime_id.as_str())
                    {
                        return Err(StoreError::ConversationOpeningLost {
                            id: opening.session_id.clone(),
                        });
                    }
                    let epoch = loaded_generation(state.conversation_epoch)?;
                    if epoch == 0 {
                        return Err(StoreError::Malformed(
                            "published conversation link has a zero epoch".into(),
                        ));
                    }
                    return Ok((
                        CoordinationOutcome::ConversationLink(ConversationLink {
                            session_id: opening.session_id.clone(),
                            epoch,
                            token: opening.token.clone(),
                            runtime_generation: opening.runtime_generation,
                            runtime_id: opening.runtime_id.clone(),
                        }),
                        Vec::new(),
                    ));
                }
                let matches = state
                    .conversation_opening
                    .as_ref()
                    .map_or(Ok(false), |held| held.matches(opening))?;
                if !matches {
                    return Err(StoreError::ConversationOpeningLost {
                        id: opening.session_id.clone(),
                    });
                }
            }
            require_conversation_backing(session, opening, session_lease.as_ref(), now)?;
            if current_generation != opening.runtime_generation
                || session.runtime_id.as_deref() != Some(opening.runtime_id.as_str())
            {
                return Err(StoreError::ConversationOpeningLost {
                    id: opening.session_id.clone(),
                });
            }
            let state = session
                .coordination
                .as_mut()
                .expect("the conversation opening coordination state was validated");
            let epoch = loaded_generation(state.conversation_epoch)?
                .checked_add(1)
                .ok_or_else(|| StoreError::Malformed("conversation link epoch overflow".into()))?;
            state.conversation_epoch = stored_generation(epoch)?;
            state.conversation_link_token = Some(opening.token.clone());
            state.conversation_opening = None;
            Ok((
                CoordinationOutcome::ConversationLink(ConversationLink {
                    session_id: opening.session_id.clone(),
                    epoch,
                    token: opening.token.clone(),
                    runtime_generation: opening.runtime_generation,
                    runtime_id: opening.runtime_id.clone(),
                }),
                Vec::new(),
            ))
        }
        CoordinationCommand::ReleaseConversationOpening(opening) => {
            let state = session
                .coordination
                .get_or_insert_with(CoordinationDoc::default);
            expire_conversation_opening(state, now);
            let matches = state
                .conversation_opening
                .as_ref()
                .map_or(Ok(false), |held| held.matches(opening))?;
            if matches {
                state.conversation_opening = None;
            }
            if state.conversation_link_token.as_deref() == Some(opening.token.as_str()) {
                state.conversation_link_token = None;
            }
            Ok((CoordinationOutcome::Unit, Vec::new()))
        }
        CoordinationCommand::Reserve { lease, token } => {
            if token.is_empty() {
                return Err(StoreError::Malformed(
                    "runtime reservation token must not be empty".into(),
                ));
            }
            require_current_lease(session, lease, now)?;
            let receipt_id =
                CoordinationReceiptIdDoc::new(&lease.session_id, RECEIPT_RESERVATION, token);
            if let Some(existing) = receipt(receipts, &receipt_id)? {
                let CoordinationReceiptPayloadDoc::Reservation(existing) = &existing.payload else {
                    return Err(StoreError::Malformed(
                        "MongoDB reservation receipt carries another payload kind".into(),
                    ));
                };
                if existing.lease_token != lease.token {
                    return Err(StoreError::RuntimeReservationChanged {
                        id: lease.session_id.clone(),
                    });
                }
                let generation = NonZeroU64::new(loaded_generation(existing.generation)?)
                    .ok_or_else(|| {
                        StoreError::Malformed("runtime reservation generation is zero".into())
                    })?;
                let next_generation = session
                    .coordination
                    .as_ref()
                    .map_or(Ok(0), |state| loaded_generation(state.next_generation))?;
                if generation.get() > next_generation {
                    return Err(StoreError::Malformed(
                        "runtime reservation is ahead of the stored generation floor".into(),
                    ));
                }
                return Ok((
                    CoordinationOutcome::Reservation(RuntimeReservation {
                        session_id: lease.session_id.clone(),
                        generation,
                        token: token.clone(),
                    }),
                    Vec::new(),
                ));
            }
            let state = session
                .coordination
                .as_mut()
                .expect("the current lease exists");
            let floor = loaded_generation(state.next_generation)?
                .max(loaded_generation(state.authority_generation)?);
            let generation = floor
                .checked_add(1)
                .and_then(NonZeroU64::new)
                .ok_or_else(|| StoreError::Malformed("runtime generation overflow".into()))?;
            state.next_generation = stored_generation(generation.get())?;
            let reservation = RuntimeReservation {
                session_id: lease.session_id.clone(),
                generation,
                token: token.clone(),
            };
            Ok((
                CoordinationOutcome::Reservation(reservation),
                vec![ReceiptMutation::Insert(CoordinationReceiptDoc::new(
                    receipt_id,
                    CoordinationReceiptPayloadDoc::Reservation(ReservationReceiptDoc {
                        lease_token: lease.token.clone(),
                        generation: stored_generation(generation.get())?,
                        runtime_id: None,
                        switch: None,
                    }),
                ))],
            ))
        }
        CoordinationCommand::Register {
            lease,
            reservation,
            runtime_id,
        } => {
            if runtime_id.is_empty() {
                return Err(StoreError::Malformed(
                    "runtime candidate handle must not be empty".into(),
                ));
            }
            require_current_lease(session, lease, now)?;
            if reservation.session_id != lease.session_id {
                return Err(StoreError::RuntimeReservationChanged {
                    id: lease.session_id.clone(),
                });
            }
            let reservation_id = CoordinationReceiptIdDoc::new(
                &lease.session_id,
                RECEIPT_RESERVATION,
                &reservation.token,
            );
            let existing = receipt(receipts, &reservation_id)?.ok_or_else(|| {
                StoreError::RuntimeReservationChanged {
                    id: lease.session_id.clone(),
                }
            })?;
            let CoordinationReceiptPayloadDoc::Reservation(mut stored) = existing.payload.clone()
            else {
                return Err(StoreError::Malformed(
                    "MongoDB reservation receipt carries another payload kind".into(),
                ));
            };
            if stored.lease_token != lease.token
                || loaded_generation(stored.generation)? != reservation.generation.get()
            {
                return Err(StoreError::RuntimeReservationChanged {
                    id: lease.session_id.clone(),
                });
            }
            if reservation.generation.get()
                > session
                    .coordination
                    .as_ref()
                    .map_or(Ok(0), |state| loaded_generation(state.next_generation))?
            {
                return Err(StoreError::Malformed(
                    "runtime reservation is ahead of the stored generation floor".into(),
                ));
            }
            let candidate = RuntimeCandidate {
                session_id: lease.session_id.clone(),
                generation: reservation.generation,
                runtime_id: runtime_id.clone(),
                token: reservation.token.clone(),
            };
            if let Some(existing_runtime) = stored.runtime_id.as_ref() {
                if existing_runtime != runtime_id {
                    return Err(StoreError::RuntimeCandidateChanged {
                        id: lease.session_id.clone(),
                    });
                }
                if stored.switch.is_none() {
                    let cleanup_id = CoordinationReceiptIdDoc::new(
                        &lease.session_id,
                        RECEIPT_CLEANUP,
                        &candidate_cleanup_token(&reservation.token),
                    );
                    let cleanup = receipt(receipts, &cleanup_id)?.ok_or_else(|| {
                        StoreError::RuntimeCandidateChanged {
                            id: lease.session_id.clone(),
                        }
                    })?;
                    let expected = RuntimeCleanup {
                        session_id: lease.session_id.clone(),
                        generation: reservation.generation.get(),
                        runtime_id: runtime_id.clone(),
                        token: cleanup_id.token,
                        reason: RuntimeCleanupReason::UncommittedCandidate,
                    };
                    if cleanup_from_receipt(cleanup)? != expected {
                        return Err(StoreError::RuntimeCandidateChanged {
                            id: lease.session_id.clone(),
                        });
                    }
                }
                return Ok((CoordinationOutcome::Candidate(candidate), Vec::new()));
            }
            if session.runtime_id.as_deref() == Some(runtime_id.as_str()) {
                return Err(StoreError::RuntimeCandidateChanged {
                    id: lease.session_id.clone(),
                });
            }
            let cleanup = RuntimeCleanup {
                session_id: lease.session_id.clone(),
                generation: reservation.generation.get(),
                runtime_id: runtime_id.clone(),
                token: candidate_cleanup_token(&reservation.token),
                reason: RuntimeCleanupReason::UncommittedCandidate,
            };
            let cleanup_id =
                CoordinationReceiptIdDoc::new(&lease.session_id, RECEIPT_CLEANUP, &cleanup.token);
            if receipt(receipts, &cleanup_id)?.is_some() {
                return Err(StoreError::RuntimeCandidateChanged {
                    id: lease.session_id.clone(),
                });
            }
            stored.runtime_id = Some(runtime_id.clone());
            Ok((
                CoordinationOutcome::Candidate(candidate),
                vec![
                    ReceiptMutation::Replace(CoordinationReceiptDoc::new(
                        reservation_id,
                        CoordinationReceiptPayloadDoc::Reservation(stored),
                    )),
                    ReceiptMutation::Insert(cleanup_receipt(&lease.session_id, &cleanup)?),
                ],
            ))
        }
        CoordinationCommand::Switch { lease, candidate } => {
            if candidate.session_id != lease.session_id {
                return Err(StoreError::RuntimeCandidateChanged {
                    id: lease.session_id.clone(),
                });
            }
            let reservation_id = CoordinationReceiptIdDoc::new(
                &lease.session_id,
                RECEIPT_RESERVATION,
                &candidate.token,
            );
            let existing = receipt(receipts, &reservation_id)?.ok_or_else(|| {
                StoreError::RuntimeCandidateChanged {
                    id: lease.session_id.clone(),
                }
            })?;
            let CoordinationReceiptPayloadDoc::Reservation(mut stored) = existing.payload.clone()
            else {
                return Err(StoreError::Malformed(
                    "MongoDB reservation receipt carries another payload kind".into(),
                ));
            };
            if stored.lease_token != lease.token
                || loaded_generation(stored.generation)? != candidate.generation.get()
                || stored.runtime_id.as_deref() != Some(candidate.runtime_id.as_str())
            {
                return Err(StoreError::RuntimeCandidateChanged {
                    id: lease.session_id.clone(),
                });
            }
            if candidate.generation.get()
                > session
                    .coordination
                    .as_ref()
                    .map_or(Ok(0), |state| loaded_generation(state.next_generation))?
            {
                return Err(StoreError::Malformed(
                    "runtime candidate is ahead of the stored generation floor".into(),
                ));
            }
            if let Some(completed) = stored.switch.as_ref() {
                let outcome_boundary = completed.boundary.to_core()?;
                let current = current_boundary(session)?;
                let state = session
                    .coordination
                    .get_or_insert_with(CoordinationDoc::default);
                expire_lease(state, now);
                let active_valid = match state.lease.as_ref() {
                    Some(active) => {
                        active.same_identity(lease)? && active.boundary == completed.boundary
                    }
                    None => false,
                };
                let valid = active_valid && current == outcome_boundary;
                if !valid {
                    return Err(StoreError::RuntimeCandidateChanged {
                        id: lease.session_id.clone(),
                    });
                }
                let cleanup = completed
                    .cleanup
                    .as_ref()
                    .map(|cleanup| {
                        cleanup.to_core(&lease.session_id, retired_cleanup_token(&candidate.token))
                    })
                    .transpose()?;
                return Ok((
                    CoordinationOutcome::Switch(RuntimeAuthoritySwitch {
                        revision: loaded_generation(completed.revision)?,
                        runtime_started_seq: loaded_generation(completed.runtime_started_seq)?,
                        boundary: outcome_boundary,
                        cleanup,
                    }),
                    Vec::new(),
                ));
            }
            require_current_lease(session, lease, now)?;
            let authority = runtime_authority_from_doc(session)?;
            if candidate.generation.get() <= authority.generation {
                return Err(StoreError::RuntimeCandidateChanged {
                    id: lease.session_id.clone(),
                });
            }
            let candidate_cleanup_id = CoordinationReceiptIdDoc::new(
                &lease.session_id,
                RECEIPT_CLEANUP,
                &candidate_cleanup_token(&candidate.token),
            );
            let candidate_cleanup = receipt(receipts, &candidate_cleanup_id)?.ok_or_else(|| {
                StoreError::RuntimeCandidateChanged {
                    id: lease.session_id.clone(),
                }
            })?;
            let expected_candidate_cleanup = RuntimeCleanup {
                session_id: lease.session_id.clone(),
                generation: candidate.generation.get(),
                runtime_id: candidate.runtime_id.clone(),
                token: candidate_cleanup_id.token.clone(),
                reason: RuntimeCleanupReason::UncommittedCandidate,
            };
            let candidate_cleanup_active = matches!(
                &candidate_cleanup.payload,
                CoordinationReceiptPayloadDoc::Cleanup(stored) if stored.completed_by.is_none()
            );
            if !candidate_cleanup_active
                || cleanup_from_receipt(candidate_cleanup)? != expected_candidate_cleanup
            {
                return Err(StoreError::RuntimeCandidateChanged {
                    id: lease.session_id.clone(),
                });
            }
            let previous_cleanup = authority
                .runtime_id
                .as_ref()
                .map(|runtime_id| RuntimeCleanup {
                    session_id: lease.session_id.clone(),
                    generation: authority.generation,
                    runtime_id: runtime_id.clone(),
                    token: retired_cleanup_token(&candidate.token),
                    reason: RuntimeCleanupReason::RetiredAuthority,
                });
            if let Some(cleanup) = previous_cleanup.as_ref() {
                let previous_id = CoordinationReceiptIdDoc::new(
                    &lease.session_id,
                    RECEIPT_CLEANUP,
                    &cleanup.token,
                );
                if receipt(receipts, &previous_id)?.is_some() {
                    return Err(StoreError::RuntimeCleanupChanged {
                        id: lease.session_id.clone(),
                    });
                }
            }
            let (runtime_started_seq, stored_runtime_started_seq) =
                next_runtime_started_sequence(runtime_started_sequence_floor)?;
            let revision = next_revision(session)?;
            session.last_seq = stored_runtime_started_seq;
            session.runtime_id = Some(candidate.runtime_id.clone());
            let boundary = SessionBoundary {
                status: parse_plain(&session.status)?,
                authority: RuntimeAuthority {
                    generation: candidate.generation.get(),
                    runtime_id: Some(candidate.runtime_id.clone()),
                },
            };
            let boundary_doc = BoundaryDoc::from_core(&boundary)?;
            let state = session
                .coordination
                .as_mut()
                .expect("the current lease exists");
            state.authority_generation = stored_generation(candidate.generation.get())?;
            state.next_generation = state
                .next_generation
                .max(stored_generation(candidate.generation.get())?);
            state
                .lease
                .as_mut()
                .expect("the current lease was validated")
                .boundary = boundary_doc.clone();
            let completed = AuthoritySwitchDoc {
                revision: stored_generation(revision)?,
                runtime_started_seq: stored_runtime_started_seq,
                boundary: boundary_doc,
                cleanup: previous_cleanup
                    .as_ref()
                    .map(CleanupDoc::from_core)
                    .transpose()?,
            };
            stored.switch = Some(completed);
            let mut mutations = vec![
                ReceiptMutation::Replace(CoordinationReceiptDoc::new(
                    reservation_id,
                    CoordinationReceiptPayloadDoc::Reservation(stored),
                )),
                ReceiptMutation::Delete(candidate_cleanup_id),
            ];
            if let Some(cleanup) = previous_cleanup.as_ref() {
                mutations.push(ReceiptMutation::Insert(cleanup_receipt(
                    &lease.session_id,
                    cleanup,
                )?));
            }
            Ok((
                CoordinationOutcome::Switch(RuntimeAuthoritySwitch {
                    revision,
                    runtime_started_seq,
                    boundary,
                    cleanup: previous_cleanup,
                }),
                mutations,
            ))
        }
        CoordinationCommand::Retire { lease, token } => {
            if token.is_empty() {
                return Err(StoreError::Malformed(
                    "runtime retirement token must not be empty".into(),
                ));
            }
            let retirement_id =
                CoordinationReceiptIdDoc::new(&lease.session_id, RECEIPT_RETIREMENT, token);
            if let Some(existing) = receipt(receipts, &retirement_id)? {
                let CoordinationReceiptPayloadDoc::Retirement(existing) = &existing.payload else {
                    return Err(StoreError::Malformed(
                        "MongoDB retirement receipt carries another payload kind".into(),
                    ));
                };
                let outcome_boundary = existing.outcome.boundary.to_core()?;
                let current = current_boundary(session)?;
                let state = session
                    .coordination
                    .get_or_insert_with(CoordinationDoc::default);
                expire_lease(state, now);
                let input_boundary = BoundaryDoc::from_core(&lease.boundary)?;
                let valid = existing.lease_token == lease.token
                    && (input_boundary == existing.original_boundary
                        || input_boundary == existing.outcome.boundary)
                    && match state.lease.as_ref() {
                        Some(active) => {
                            active.same_identity(lease)?
                                && active.boundary == existing.outcome.boundary
                        }
                        None => false,
                    }
                    && current == outcome_boundary;
                if !valid {
                    return Err(StoreError::RuntimeCleanupChanged {
                        id: lease.session_id.clone(),
                    });
                }
                return Ok((
                    CoordinationOutcome::Retirement(RuntimeAuthorityRetirement {
                        revision: loaded_generation(existing.outcome.revision)?,
                        boundary: outcome_boundary,
                        cleanup: existing
                            .outcome
                            .cleanup
                            .to_core(&lease.session_id, retired_cleanup_token(token))?,
                    }),
                    Vec::new(),
                ));
            }
            require_current_lease(session, lease, now)?;
            if !matches!(
                lease.operation,
                SessionOperation::Recover | SessionOperation::Lifecycle
            ) {
                return Err(StoreError::SessionLeaseLost {
                    id: lease.session_id.clone(),
                });
            }
            let authority = runtime_authority_from_doc(session)?;
            let Some(runtime_id) = authority.runtime_id else {
                return Err(StoreError::RuntimeCleanupChanged {
                    id: lease.session_id.clone(),
                });
            };
            let cleanup = RuntimeCleanup {
                session_id: lease.session_id.clone(),
                generation: authority.generation,
                runtime_id,
                token: retired_cleanup_token(token),
                reason: RuntimeCleanupReason::RetiredAuthority,
            };
            let cleanup_id =
                CoordinationReceiptIdDoc::new(&lease.session_id, RECEIPT_CLEANUP, &cleanup.token);
            if receipt(receipts, &cleanup_id)?.is_some() {
                return Err(StoreError::RuntimeCleanupChanged {
                    id: lease.session_id.clone(),
                });
            }
            let original_boundary = current_boundary(session)?;
            let revision = next_revision(session)?;
            session.runtime_id = None;
            let boundary = SessionBoundary {
                status: parse_plain(&session.status)?,
                authority: RuntimeAuthority {
                    generation: cleanup.generation,
                    runtime_id: None,
                },
            };
            let boundary_doc = BoundaryDoc::from_core(&boundary)?;
            session
                .coordination
                .as_mut()
                .and_then(|state| state.lease.as_mut())
                .expect("the current lease was validated")
                .boundary = boundary_doc.clone();
            let outcome_doc = AuthorityRetirementDoc {
                revision: stored_generation(revision)?,
                boundary: boundary_doc,
                cleanup: CleanupDoc::from_core(&cleanup)?,
            };
            Ok((
                CoordinationOutcome::Retirement(RuntimeAuthorityRetirement {
                    revision,
                    boundary,
                    cleanup: cleanup.clone(),
                }),
                vec![
                    ReceiptMutation::Insert(CoordinationReceiptDoc::new(
                        retirement_id,
                        CoordinationReceiptPayloadDoc::Retirement(RetirementReceiptDoc {
                            lease_token: lease.token.clone(),
                            original_boundary: BoundaryDoc::from_core(&original_boundary)?,
                            outcome: outcome_doc,
                        }),
                    )),
                    ReceiptMutation::Insert(cleanup_receipt(&lease.session_id, &cleanup)?),
                ],
            ))
        }
        CoordinationCommand::Acknowledge { lease, cleanup } => {
            require_current_lease(session, lease, now)?;
            if !matches!(
                lease.operation,
                SessionOperation::Reclaim | SessionOperation::Recover | SessionOperation::Lifecycle
            ) || cleanup.session_id != lease.session_id
            {
                return Err(StoreError::SessionLeaseLost {
                    id: lease.session_id.clone(),
                });
            }
            let authority = runtime_authority_from_doc(session)?;
            if authority.generation == cleanup.generation
                && authority.runtime_id.as_deref() == Some(cleanup.runtime_id.as_str())
            {
                return Err(StoreError::RuntimeCleanupChanged {
                    id: lease.session_id.clone(),
                });
            }
            let cleanup_id =
                CoordinationReceiptIdDoc::new(&lease.session_id, RECEIPT_CLEANUP, &cleanup.token);
            let existing = receipt(receipts, &cleanup_id)?.ok_or_else(|| {
                StoreError::RuntimeCleanupChanged {
                    id: lease.session_id.clone(),
                }
            })?;
            let CoordinationReceiptPayloadDoc::Cleanup(mut stored) = existing.payload.clone()
            else {
                return Err(StoreError::Malformed(
                    "MongoDB cleanup receipt carries another payload kind".into(),
                ));
            };
            if cleanup_from_receipt(existing)? != *cleanup {
                return Err(StoreError::RuntimeCleanupChanged {
                    id: lease.session_id.clone(),
                });
            }
            if stored.completed_by.is_some() {
                return Ok((CoordinationOutcome::Unit, Vec::new()));
            }
            stored.completed_by = Some(lease.token.clone());
            Ok((
                CoordinationOutcome::Unit,
                vec![ReceiptMutation::Replace(CoordinationReceiptDoc::new(
                    cleanup_id,
                    CoordinationReceiptPayloadDoc::Cleanup(stored),
                ))],
            ))
        }
    }
}

impl MongoSessionStore {
    pub(super) async fn validate_coordination_receipts(&self) -> Result<(), StoreError> {
        let context = (
            self.sessions.clone(),
            self.events.clone(),
            self.receipts.clone_with_type::<Document>(),
        );
        let mut client_session = self.client.start_session().await.map_err(|error| {
            StoreError::Unavailable(format!("start MongoDB coordination receipt audit: {error}"))
        })?;
        client_session
            .start_transaction()
            .read_concern(ReadConcern::snapshot())
            .write_concern(WriteConcern::majority())
            .and_run(
                context,
                |client_session, (sessions_collection, events_collection, receipts_collection)| {
                    async move {
                        let mut session_cursor = sessions_collection
                            .find(doc! {})
                            .session(&mut *client_session)
                            .await?;
                        let mut sessions = BTreeMap::new();
                        while let Some(session) = session_cursor
                            .next(&mut *client_session)
                            .await
                            .transpose()?
                        {
                            sessions.insert(session.id.clone(), session);
                        }

                        let mut receipt_cursor = receipts_collection
                            .find(doc! {})
                            .session(&mut *client_session)
                            .await?;
                        let mut documents = Vec::new();
                        while let Some(document) = receipt_cursor
                            .next(&mut *client_session)
                            .await
                            .transpose()?
                        {
                            documents.push(document);
                        }
                        let receipts = coordination_receipt_map(documents)
                            .map_err(mongodb::error::Error::custom)?;
                        validate_coordination_receipt_set(&sessions, &receipts)
                            .map_err(mongodb::error::Error::custom)?;
                        let expected_markers = runtime_started_marker_expectations(&receipts)
                            .map_err(mongodb::error::Error::custom)?;
                        let marker_batches =
                            runtime_started_marker_audit_batches(&expected_markers)
                                .map_err(mongodb::error::Error::custom)?;
                        // Every `$or` clause is an exact prefix-complete lookup
                        // through the required unique (session_id, seq) index.
                        // Keep all bounded reads in this snapshot transaction so
                        // a concurrent atomic switch is observed wholly before
                        // or after it.
                        for batch in marker_batches {
                            let mut marker_cursor = events_collection
                                .find(batch.filter.clone())
                                .session(&mut *client_session)
                                .await?;
                            let mut markers = Vec::with_capacity(batch.expectations.len());
                            while let Some(marker) =
                                marker_cursor.next(&mut *client_session).await.transpose()?
                            {
                                markers.push(marker);
                            }
                            validate_runtime_started_marker_batch(&batch, markers)
                                .map_err(mongodb::error::Error::custom)?;
                        }
                        Ok(())
                    }
                    .boxed()
                },
            )
            .await
            .map_err(|error| {
                error
                    .get_custom::<StoreError>()
                    .cloned()
                    .unwrap_or_else(|| {
                        StoreError::Unavailable(format!(
                            "audit MongoDB coordination receipts: {error}"
                        ))
                    })
            })
    }

    pub(super) async fn run_coordination_command(
        &self,
        command: CoordinationCommand,
    ) -> Result<CoordinationOutcome, StoreError> {
        let mut last = None;
        for _ in 0..2 {
            match self.run_coordination_command_once(command.clone()).await {
                Ok(outcome) => return Ok(outcome),
                Err(StoreError::Unavailable(error)) => last = Some(error),
                Err(error) => return Err(error),
            }
        }
        Err(StoreError::Unavailable(last.unwrap_or_else(|| {
            format!("{} transaction did not complete", command.name())
        })))
    }

    async fn run_coordination_command_once(
        &self,
        command: CoordinationCommand,
    ) -> Result<CoordinationOutcome, StoreError> {
        let id = command.id().clone();
        let name = command.name();
        let context = (
            self.sessions.clone(),
            self.events.clone(),
            self.receipts.clone(),
            command,
        );
        let mut client_session = self.client.start_session().await.map_err(|error| {
            StoreError::Unavailable(format!("start {name} transaction: {error}"))
        })?;
        let outcome = client_session
            .start_transaction()
            .read_concern(ReadConcern::snapshot())
            .write_concern(WriteConcern::majority())
            .and_run(
                context,
                |client_session, (sessions, events, receipts, command)| {
                    async move {
                        // Every coordination transaction writes this session document, so
                        // overlapping attempts conflict and serialize. A successful retry
                        // with the same nonce is therefore ordered after any earlier staged
                        // write for that nonce; the caller can exact-release that result.
                        let clocked = sessions
                            .update_one(
                                doc! { "_id": &command.id().0 },
                                coordination_clock_update(),
                            )
                            .session(&mut *client_session)
                            .await?;
                        if clocked.matched_count != 1 {
                            return Err(mongodb::error::Error::custom(
                                StoreError::SessionDeleted {
                                    id: command.id().clone(),
                                },
                            ));
                        }
                        let mut stored = sessions
                            .find_one(doc! { "_id": &command.id().0 })
                            .session(&mut *client_session)
                            .await?
                            .ok_or_else(|| {
                                mongodb::error::Error::custom(StoreError::SessionDeleted {
                                    id: command.id().clone(),
                                })
                            })?;
                        let runtime_started_sequence_floor =
                            if matches!(&command, CoordinationCommand::Switch { .. }) {
                                let event_tail = events
                                    .find_one(doc! { "session_id": &command.id().0 })
                                    .sort(doc! { "seq": -1 })
                                    .projection(doc! { "_id": 0, "seq": 1 })
                                    .session(&mut *client_session)
                                    .await?;
                                let event_tail = super::stored_event_tail(event_tail)
                                    .map_err(mongodb::error::Error::custom)?;
                                super::durable_sequence_floor(event_tail, Some(stored.last_seq))
                                    .map_err(mongodb::error::Error::custom)?
                            } else {
                                u64::try_from(stored.last_seq).map_err(|_| {
                                    mongodb::error::Error::custom(StoreError::Malformed(
                                        "session event tail must not be negative".into(),
                                    ))
                                })?
                            };
                        let mut found = Vec::new();
                        for receipt_id in command.receipt_ids() {
                            if let Some(receipt) = receipts
                                .find_one(receipt_id.filter())
                                .session(&mut *client_session)
                                .await?
                            {
                                found.push(receipt);
                            }
                        }
                        let (outcome, mutations, runtime_started) =
                            apply_command_at_sequence_floor(
                                &mut stored,
                                command,
                                &found,
                                runtime_started_sequence_floor,
                            )
                            .map_err(mongodb::error::Error::custom)?;
                        for mutation in mutations {
                            match mutation {
                                ReceiptMutation::Insert(receipt) => {
                                    receipts
                                        .insert_one(receipt)
                                        .session(&mut *client_session)
                                        .await?;
                                }
                                ReceiptMutation::Replace(receipt) => {
                                    let filter = receipt.id.filter();
                                    let replaced = receipts
                                        .replace_one(filter, receipt)
                                        .with_options(
                                            ReplaceOptions::builder().upsert(false).build(),
                                        )
                                        .session(&mut *client_session)
                                        .await?;
                                    if replaced.matched_count != 1 {
                                        return Err(mongodb::error::Error::custom(
                                            StoreError::SessionAuthorityChanged {
                                                id: command.id().clone(),
                                            },
                                        ));
                                    }
                                }
                                ReceiptMutation::Delete(receipt_id) => {
                                    let deleted = receipts
                                        .delete_one(receipt_id.filter())
                                        .session(&mut *client_session)
                                        .await?;
                                    if deleted.deleted_count != 1 {
                                        return Err(mongodb::error::Error::custom(
                                            StoreError::RuntimeCleanupChanged {
                                                id: command.id().clone(),
                                            },
                                        ));
                                    }
                                }
                            }
                        }
                        if let Some(runtime_started) = runtime_started {
                            let document = super::event_doc::to_document(&runtime_started)
                                .map_err(mongodb::error::Error::custom)?;
                            events
                                .insert_one(document)
                                .session(&mut *client_session)
                                .await?;
                        }
                        let replaced = sessions
                            .replace_one(doc! { "_id": &stored.id }, stored)
                            .with_options(ReplaceOptions::builder().upsert(false).build())
                            .session(&mut *client_session)
                            .await?;
                        if replaced.matched_count != 1 {
                            return Err(mongodb::error::Error::custom(
                                StoreError::SessionDeleted {
                                    id: command.id().clone(),
                                },
                            ));
                        }
                        Ok(outcome)
                    }
                    .boxed()
                },
            )
            .await;
        match outcome {
            Ok(outcome) => Ok(outcome),
            Err(error) => {
                if let Some(error) = error.get_custom::<StoreError>() {
                    return Err(error.clone());
                }
                Err(StoreError::Unavailable(format!(
                    "{name} transaction for {id}: {error}"
                )))
            }
        }
    }

    pub(super) async fn mongo_runtime_cleanup_debts(
        &self,
        id: &layer2_core::SessionId,
    ) -> Result<Vec<RuntimeCleanup>, StoreError> {
        if self.mongo_session_identity(id).await?.is_none() {
            return Ok(Vec::new());
        }
        let mut cursor = self
            .receipts
            .find(active_cleanup_filter(id))
            .await
            .map_err(|error| {
                StoreError::Unavailable(format!("read runtime cleanup debts: {error}"))
            })?;
        let mut cleanups = Vec::new();
        while let Some(receipt) = cursor.try_next().await.map_err(|error| {
            StoreError::Unavailable(format!("runtime cleanup debt cursor: {error}"))
        })? {
            cleanups.push(cleanup_from_receipt(&receipt)?);
        }
        cleanups.sort_by(|left, right| left.token.cmp(&right.token));
        Ok(cleanups)
    }

    /// Read only the identity needed to distinguish an absent session before
    /// querying its cleanup receipts. Session documents can carry staged skill
    /// bodies, none of which belongs in this per-sweep existence check.
    pub(super) async fn mongo_session_identity(
        &self,
        id: &layer2_core::SessionId,
    ) -> Result<Option<Document>, StoreError> {
        self.sessions
            .clone_with_type::<Document>()
            .find_one(doc! { "_id": &id.0 })
            .projection(session_identity_projection())
            .await
            .map_err(|error| {
                StoreError::Unavailable(format!("read session before cleanup debt: {error}"))
            })
    }
}

fn session_identity_projection() -> Document {
    doc! { "_id": 1_i32 }
}

pub(super) fn active_cleanup_filter(id: &layer2_core::SessionId) -> Document {
    doc! {
        "_id.session_id": &id.0,
        "_id.kind": RECEIPT_CLEANUP,
        "payload.type": "cleanup",
        "$or": [
            { "payload.value.completed_by": { "$exists": false } },
            { "payload.value.completed_by": Bson::Null },
        ],
    }
}

pub(super) fn all_session_receipts_filter(id: &layer2_core::SessionId) -> Document {
    doc! { "_id.session_id": &id.0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use layer2_core::{
        AgentOptions, KernelId, RecoveryState, Session, SessionId, SessionRecord, SessionStatus,
        SubjectId, TerminalGeometry,
    };

    const NOW: i64 = 1_800_000_000_000;

    fn session_doc() -> SessionDoc {
        let mut record = SessionRecord::new(Session {
            id: SessionId("sesn_mongo_coordination".into()),
            owner: SubjectId("subject-mongo-coordination".into()),
            kernel: KernelId::new("example-runtime"),
            status: SessionStatus::Ready,
            recovery_state: RecoveryState::Normal,
            stop_reason: None,
            options: AgentOptions::default(),
            environment_profile: None,
            geometry: TerminalGeometry::default(),
            last_seq: 0,
            runtime_version: None,
            rotations: 0,
            rotated_at: None,
            capabilities: Default::default(),
            granted_services: None,
            granted_skills: None,
            pricing: None,
            created_at: "2027-01-15T08:00:00Z".into(),
        });
        record.runtime_id = Some("runtime-original".into());
        record.revision = 4;
        let mut stored = super::super::to_doc(&record).expect("session can be stored");
        stored.coordination = Some(CoordinationDoc {
            authority_generation: 1,
            next_generation: 1,
            receipt_count: 0,
            interaction_epoch: 0,
            conversation_epoch: 0,
            conversation_link_token: None,
            conversation_opening: None,
            lease: None,
            server_now: Some(DateTime::from_millis(NOW)),
        });
        stored
    }

    fn request(session: &SessionDoc, token: &str) -> SessionLeaseRequest {
        SessionLeaseRequest {
            session_id: SessionId(session.id.clone()),
            holder: "mongo-test-holder".into(),
            token: token.into(),
            boundary: current_boundary(session).expect("boundary is valid"),
            operation: SessionOperation::Lifecycle,
            ttl: std::time::Duration::from_secs(30),
        }
    }

    fn interaction_request(
        session: &SessionDoc,
        nonce: impl Into<String>,
    ) -> SessionInteractionLeaseRequest {
        SessionInteractionLeaseRequest {
            session_id: SessionId(session.id.clone()),
            holder: "mongo-interaction-holder".into(),
            nonce: nonce.into(),
            boundary: current_boundary(session).expect("boundary is valid"),
            ttl: std::time::Duration::from_secs(30),
        }
    }

    fn apply_mutations(
        receipts: &mut Vec<CoordinationReceiptDoc>,
        mutations: Vec<ReceiptMutation>,
    ) {
        for mutation in mutations {
            match mutation {
                ReceiptMutation::Insert(receipt) => {
                    assert!(
                        !receipts.iter().any(|stored| stored.id == receipt.id),
                        "test state cannot insert a duplicate receipt"
                    );
                    receipts.push(receipt);
                }
                ReceiptMutation::Replace(receipt) => {
                    let stored = receipts
                        .iter_mut()
                        .find(|stored| stored.id == receipt.id)
                        .expect("replacement receipt exists");
                    *stored = receipt;
                }
                ReceiptMutation::Delete(id) => {
                    let before = receipts.len();
                    receipts.retain(|stored| stored.id != id);
                    assert_eq!(receipts.len() + 1, before, "deleted receipt exists");
                }
            }
        }
    }

    fn run(
        session: &mut SessionDoc,
        receipts: &mut Vec<CoordinationReceiptDoc>,
        command: CoordinationCommand,
    ) -> Result<CoordinationOutcome, StoreError> {
        run_with_runtime_started(session, receipts, command).map(|(outcome, _)| outcome)
    }

    fn run_with_runtime_started(
        session: &mut SessionDoc,
        receipts: &mut Vec<CoordinationReceiptDoc>,
        command: CoordinationCommand,
    ) -> Result<(CoordinationOutcome, Option<Event>), StoreError> {
        let sequence_floor = u64::try_from(session.last_seq)
            .map_err(|_| StoreError::Malformed("session event tail must not be negative".into()))?;
        run_with_runtime_started_at_sequence_floor(session, receipts, command, sequence_floor)
    }

    fn run_with_runtime_started_at_sequence_floor(
        session: &mut SessionDoc,
        receipts: &mut Vec<CoordinationReceiptDoc>,
        command: CoordinationCommand,
        sequence_floor: u64,
    ) -> Result<(CoordinationOutcome, Option<Event>), StoreError> {
        session
            .coordination
            .get_or_insert_with(CoordinationDoc::default)
            .server_now = Some(DateTime::from_millis(NOW));
        let (outcome, mutations, runtime_started) =
            apply_command_at_sequence_floor(session, &command, receipts, sequence_floor)?;
        apply_mutations(receipts, mutations);
        Ok((outcome, runtime_started))
    }

    fn active_cleanups(receipts: &[CoordinationReceiptDoc]) -> Vec<RuntimeCleanup> {
        let mut cleanups = receipts
            .iter()
            .filter_map(|receipt| match &receipt.payload {
                CoordinationReceiptPayloadDoc::Cleanup(stored) if stored.completed_by.is_none() => {
                    Some(cleanup_from_receipt(receipt).expect("cleanup receipt is valid"))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        cleanups.sort_by(|left, right| left.token.cmp(&right.token));
        cleanups
    }

    fn validate_test_receipts(
        session: &SessionDoc,
        receipts: &[CoordinationReceiptDoc],
    ) -> Result<(), StoreError> {
        validate_coordination_receipt_set(
            &BTreeMap::from([(session.id.clone(), session.clone())]),
            &receipts
                .iter()
                .cloned()
                .map(|receipt| (receipt.id.clone(), receipt))
                .collect(),
        )
    }

    fn compatibility_session_doc() -> SessionDoc {
        let mut session = session_doc();
        let state = session.coordination.as_mut().unwrap();
        state.authority_generation = 0;
        state.next_generation = 1;
        session
    }

    fn session_bson(session: &SessionDoc) -> Document {
        mongodb::bson::serialize_to_document(session).expect("test session serializes")
    }

    fn conversation_request(
        session: &SessionDoc,
        holder: &str,
        token: &str,
        generation: u64,
        candidate: Option<&str>,
    ) -> ConversationOpeningLeaseRequest {
        ConversationOpeningLeaseRequest {
            session_id: SessionId(session.id.clone()),
            holder: holder.into(),
            token: token.into(),
            runtime_id: session.runtime_id.clone().unwrap(),
            runtime_generation: generation,
            runtime_candidate_token: candidate.map(str::to_owned),
            ttl: std::time::Duration::from_secs(30),
        }
    }

    fn acquire_conversation(
        session: &mut SessionDoc,
        receipts: &mut Vec<CoordinationReceiptDoc>,
        request: ConversationOpeningLeaseRequest,
        session_lease: Option<SessionLease>,
    ) -> Result<ConversationOpeningLease, StoreError> {
        match run(
            session,
            receipts,
            CoordinationCommand::AcquireConversationOpening {
                request,
                session_lease,
            },
        )? {
            CoordinationOutcome::ConversationOpening(opening) => Ok(opening),
            _ => panic!("conversation opening expected"),
        }
    }

    fn publish_conversation(
        session: &mut SessionDoc,
        receipts: &mut Vec<CoordinationReceiptDoc>,
        opening: ConversationOpeningLease,
        session_lease: Option<SessionLease>,
    ) -> Result<ConversationLink, StoreError> {
        match run(
            session,
            receipts,
            CoordinationCommand::PublishConversationLink {
                opening,
                session_lease,
            },
        )? {
            CoordinationOutcome::ConversationLink(link) => Ok(link),
            _ => panic!("conversation link expected"),
        }
    }

    #[test]
    fn mongo_conversation_publication_replays_supersedes_and_releases_exactly() {
        let mut session = compatibility_session_doc();
        let mut receipts = Vec::new();
        let first_request =
            conversation_request(&session, "holder-first", "opening-first", 0, None);
        let first =
            acquire_conversation(&mut session, &mut receipts, first_request.clone(), None).unwrap();
        assert_eq!(
            acquire_conversation(&mut session, &mut receipts, first_request, None).unwrap(),
            first
        );
        let held = session.clone();
        assert!(matches!(
            acquire_conversation(
                &mut session,
                &mut receipts,
                conversation_request(&held, "holder-second", "opening-second", 0, None),
                None,
            ),
            Err(StoreError::ConversationOpeningHeld { .. })
        ));
        assert_eq!(
            session_bson(&session),
            session_bson(&held),
            "rejected contention mutates no authority"
        );

        let first_link =
            publish_conversation(&mut session, &mut receipts, first.clone(), None).unwrap();
        assert_eq!(
            publish_conversation(&mut session, &mut receipts, first.clone(), None).unwrap(),
            first_link
        );
        let second_request =
            conversation_request(&session, "holder-second", "opening-second", 0, None);
        let second =
            acquire_conversation(&mut session, &mut receipts, second_request, None).unwrap();
        require_conversation_write_authority(
            &mut session,
            &ConversationWriteAuthority::Active(first_link.clone()),
        )
        .expect("a pending successor does not displace the active link");
        let second_link =
            publish_conversation(&mut session, &mut receipts, second.clone(), None).unwrap();
        assert!(second_link.epoch > first_link.epoch);
        let superseded = session.clone();
        assert!(matches!(
            require_conversation_write_authority(
                &mut session,
                &ConversationWriteAuthority::Active(first_link),
            ),
            Err(StoreError::ConversationLinkChanged { .. })
        ));
        assert_eq!(
            session_bson(&session),
            session_bson(&superseded),
            "stale active authority mutates nothing"
        );
        assert!(matches!(
            require_conversation_write_authority(
                &mut session,
                &ConversationWriteAuthority::Opening {
                    opening: first,
                    session_lease: None,
                },
            ),
            Err(StoreError::ConversationOpeningLost { .. })
        ));

        run(
            &mut session,
            &mut receipts,
            CoordinationCommand::ReleaseConversationOpening(second),
        )
        .unwrap();
        let released_epoch = session.coordination.as_ref().unwrap().conversation_epoch;
        assert!(released_epoch > 0);
        assert!(session
            .coordination
            .as_ref()
            .unwrap()
            .conversation_link_token
            .is_none());
        validate_coordination_state(&session).unwrap();
        assert!(matches!(
            require_conversation_write_authority(
                &mut session,
                &ConversationWriteAuthority::Active(second_link),
            ),
            Err(StoreError::ConversationLinkChanged { .. })
        ));

        let collision_request = conversation_request(
            &session,
            "holder-collision",
            "released:opening-second",
            0,
            None,
        );
        let collision =
            acquire_conversation(&mut session, &mut receipts, collision_request, None).unwrap();
        let collision_link =
            publish_conversation(&mut session, &mut receipts, collision, None).unwrap();
        assert!(collision_link.epoch > u64::try_from(released_epoch).unwrap());
        require_conversation_write_authority(
            &mut session,
            &ConversationWriteAuthority::Active(collision_link),
        )
        .unwrap();
    }

    #[test]
    fn mongo_nonzero_conversation_openings_require_the_exact_provider_lease() {
        let mut session = session_doc();
        let mut receipts = Vec::new();
        let no_lease =
            conversation_request(&session, "mongo-test-holder", "opening-no-lease", 1, None);
        let before = session.clone();
        assert!(matches!(
            acquire_conversation(&mut session, &mut receipts, no_lease, None),
            Err(StoreError::ConversationOpeningLost { .. })
        ));
        assert_eq!(session_bson(&session), session_bson(&before));

        let lease_request = request(&session, "lease-conversation-readoption");
        let mut lease = match run(
            &mut session,
            &mut receipts,
            CoordinationCommand::Acquire(lease_request),
        )
        .unwrap()
        {
            CoordinationOutcome::Lease(lease) => lease,
            _ => panic!("provider lease expected"),
        };
        let readoption =
            conversation_request(&session, &lease.holder, "opening-readoption", 1, None);
        let opening =
            acquire_conversation(&mut session, &mut receipts, readoption, Some(lease.clone()))
                .unwrap();
        let mut forged = lease.clone();
        forged.token.push_str("-forged");
        assert!(matches!(
            publish_conversation(&mut session, &mut receipts, opening.clone(), Some(forged),),
            Err(StoreError::ConversationOpeningLost { .. })
        ));
        let readoption_link =
            publish_conversation(&mut session, &mut receipts, opening, Some(lease.clone()))
                .unwrap();

        let forged_candidate = conversation_request(
            &session,
            &lease.holder,
            "opening-unregistered-candidate",
            2,
            Some("missing-reservation"),
        );
        let before = session.clone();
        let before_receipts = receipts.clone();
        assert!(matches!(
            acquire_conversation(
                &mut session,
                &mut receipts,
                forged_candidate,
                Some(lease.clone()),
            ),
            Err(StoreError::ConversationOpeningLost { .. })
        ));
        assert_eq!(session_bson(&session), session_bson(&before));
        assert_eq!(receipts, before_receipts);

        let reservation = match run(
            &mut session,
            &mut receipts,
            CoordinationCommand::Reserve {
                lease: lease.clone(),
                token: "candidate-conversation".into(),
            },
        )
        .unwrap()
        {
            CoordinationOutcome::Reservation(reservation) => reservation,
            _ => panic!("candidate reservation expected"),
        };
        let candidate = match run(
            &mut session,
            &mut receipts,
            CoordinationCommand::Register {
                lease: lease.clone(),
                reservation,
                runtime_id: "runtime-conversation".into(),
            },
        )
        .unwrap()
        {
            CoordinationOutcome::Candidate(candidate) => candidate,
            _ => panic!("registered candidate expected"),
        };
        let before = session.clone();
        let before_receipts = receipts.clone();

        let mut wrong_generation = conversation_request(
            &session,
            &lease.holder,
            "opening-wrong-generation",
            candidate.generation.get() + 1,
            Some(&candidate.token),
        );
        wrong_generation.runtime_id = candidate.runtime_id.clone();
        assert!(matches!(
            acquire_conversation(
                &mut session,
                &mut receipts,
                wrong_generation,
                Some(lease.clone()),
            ),
            Err(StoreError::ConversationOpeningLost { .. })
        ));
        assert_eq!(session_bson(&session), session_bson(&before));
        assert_eq!(receipts, before_receipts);

        let mut wrong_runtime = conversation_request(
            &session,
            &lease.holder,
            "opening-wrong-runtime",
            candidate.generation.get(),
            Some(&candidate.token),
        );
        wrong_runtime.runtime_id = "runtime-forged".into();
        assert!(matches!(
            acquire_conversation(
                &mut session,
                &mut receipts,
                wrong_runtime,
                Some(lease.clone()),
            ),
            Err(StoreError::ConversationOpeningLost { .. })
        ));
        assert_eq!(session_bson(&session), session_bson(&before));
        assert_eq!(receipts, before_receipts);

        let mut candidate_request = conversation_request(
            &session,
            &lease.holder,
            "opening-candidate",
            candidate.generation.get(),
            Some(&candidate.token),
        );
        candidate_request.runtime_id = candidate.runtime_id.clone();
        let candidate_opening = acquire_conversation(
            &mut session,
            &mut receipts,
            candidate_request,
            Some(lease.clone()),
        )
        .expect("the exact registered candidate acquires before the switch");
        let switched = match run(
            &mut session,
            &mut receipts,
            CoordinationCommand::Switch {
                lease: lease.clone(),
                candidate,
            },
        )
        .unwrap()
        {
            CoordinationOutcome::Switch(switched) => switched,
            _ => panic!("candidate switch expected"),
        };
        lease.boundary = switched.boundary;
        let candidate_link = publish_conversation(
            &mut session,
            &mut receipts,
            candidate_opening.clone(),
            Some(lease),
        )
        .expect("the exact candidate publishes after becoming authoritative");
        assert!(candidate_link.epoch > readoption_link.epoch);
        assert!(matches!(
            require_conversation_write_authority(
                &mut session,
                &ConversationWriteAuthority::Active(readoption_link),
            ),
            Err(StoreError::ConversationLinkChanged { .. })
        ));
        require_conversation_write_authority(
            &mut session,
            &ConversationWriteAuthority::Active(candidate_link.clone()),
        )
        .unwrap();
        run(
            &mut session,
            &mut receipts,
            CoordinationCommand::ReleaseConversationOpening(candidate_opening),
        )
        .unwrap();
        assert!(matches!(
            require_conversation_write_authority(
                &mut session,
                &ConversationWriteAuthority::Active(candidate_link),
            ),
            Err(StoreError::ConversationLinkChanged { .. })
        ));
    }

    #[test]
    fn mongo_authority_switch_and_retirement_retain_each_exact_cleanup() {
        let mut session = session_doc();
        let mut receipts = Vec::new();
        let request = request(&session, "lease-terminal-flow");
        let mut lease = match run(
            &mut session,
            &mut receipts,
            CoordinationCommand::Acquire(request),
        )
        .unwrap()
        {
            CoordinationOutcome::Lease(lease) => lease,
            _ => panic!("lease expected"),
        };
        let reservation = match run(
            &mut session,
            &mut receipts,
            CoordinationCommand::Reserve {
                lease: lease.clone(),
                token: "candidate-two".into(),
            },
        )
        .unwrap()
        {
            CoordinationOutcome::Reservation(reservation) => reservation,
            _ => panic!("reservation expected"),
        };
        let candidate = match run(
            &mut session,
            &mut receipts,
            CoordinationCommand::Register {
                lease: lease.clone(),
                reservation,
                runtime_id: "runtime-replacement".into(),
            },
        )
        .unwrap()
        {
            CoordinationOutcome::Candidate(candidate) => candidate,
            _ => panic!("candidate expected"),
        };
        let switched = match run(
            &mut session,
            &mut receipts,
            CoordinationCommand::Switch {
                lease: lease.clone(),
                candidate,
            },
        )
        .unwrap()
        {
            CoordinationOutcome::Switch(switched) => switched,
            _ => panic!("switch expected"),
        };
        lease.boundary = switched.boundary;
        assert_eq!(session.runtime_id.as_deref(), Some("runtime-replacement"));
        assert_eq!(runtime_authority_from_doc(&session).unwrap().generation, 2);
        assert_eq!(
            active_cleanups(&receipts),
            vec![RuntimeCleanup {
                session_id: lease.session_id.clone(),
                generation: 1,
                runtime_id: "runtime-original".into(),
                token: "retired:candidate-two".into(),
                reason: RuntimeCleanupReason::RetiredAuthority,
            }]
        );

        let retired = match run(
            &mut session,
            &mut receipts,
            CoordinationCommand::Retire {
                lease: lease.clone(),
                token: "terminal-retirement".into(),
            },
        )
        .unwrap()
        {
            CoordinationOutcome::Retirement(retired) => retired,
            _ => panic!("retirement expected"),
        };
        lease.boundary = retired.boundary;
        assert!(session.runtime_id.is_none());
        assert_eq!(
            active_cleanups(&receipts),
            vec![
                RuntimeCleanup {
                    session_id: lease.session_id.clone(),
                    generation: 1,
                    runtime_id: "runtime-original".into(),
                    token: "retired:candidate-two".into(),
                    reason: RuntimeCleanupReason::RetiredAuthority,
                },
                RuntimeCleanup {
                    session_id: lease.session_id.clone(),
                    generation: 2,
                    runtime_id: "runtime-replacement".into(),
                    token: "retired:terminal-retirement".into(),
                    reason: RuntimeCleanupReason::RetiredAuthority,
                },
            ]
        );
        validate_test_receipts(&session, &receipts).unwrap();
        for kind in [
            RECEIPT_LEASE,
            RECEIPT_RESERVATION,
            RECEIPT_RETIREMENT,
            RECEIPT_CLEANUP,
        ] {
            let mut missing = receipts.clone();
            let index = missing
                .iter()
                .position(|receipt| receipt.id.kind == kind)
                .expect("the completed flow stores every required receipt kind");
            missing.remove(index);
            assert!(
                validate_test_receipts(&session, &missing).is_err(),
                "missing {kind} receipt must fail the coordination audit"
            );
        }
    }

    fn marker_document(session_id: &str, seq: u64, generation: u64) -> Document {
        super::super::event_doc::to_document(&Event {
            session_id: layer2_core::SessionId(session_id.into()),
            seq,
            kind: EventKind::RuntimeStarted {
                generation: NonZeroU64::new(generation).expect("test generation is positive"),
            },
            appended_at: "2027-01-15T08:00:00Z".into(),
        })
        .expect("test marker serializes")
    }

    #[test]
    fn runtime_started_marker_audit_queries_are_exact_and_bounded() {
        assert!(runtime_started_marker_audit_batches(&BTreeMap::new())
            .unwrap()
            .is_empty());

        let marker_count = RUNTIME_STARTED_MARKER_AUDIT_BATCH_SIZE * 2 + 1;
        let expectations = (1..=marker_count)
            .map(|seq| {
                (
                    ("sesn_marker_batch".to_owned(), seq as u64),
                    NonZeroU64::new(seq as u64).unwrap(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let batches = runtime_started_marker_audit_batches(&expectations).unwrap();
        assert_eq!(batches.len(), 3);
        assert_eq!(
            batches
                .iter()
                .map(|batch| batch.expectations.len())
                .collect::<Vec<_>>(),
            vec![
                RUNTIME_STARTED_MARKER_AUDIT_BATCH_SIZE,
                RUNTIME_STARTED_MARKER_AUDIT_BATCH_SIZE,
                1,
            ]
        );
        for batch in batches {
            let clauses = batch.filter.get_array("$or").unwrap();
            assert!(!clauses.is_empty());
            assert!(clauses.len() <= RUNTIME_STARTED_MARKER_AUDIT_BATCH_SIZE);
            assert_eq!(clauses.len(), batch.expectations.len());
            for (clause, ((session_id, seq), _)) in clauses.iter().zip(&batch.expectations) {
                assert_eq!(
                    clause,
                    &Bson::Document(doc! {
                        "session_id": session_id,
                        "seq": *seq as i64,
                    })
                );
            }
        }
    }

    #[test]
    fn runtime_started_marker_batch_reconciles_every_returned_position() {
        let expectations = BTreeMap::from([
            (("sesn_marker_a".to_owned(), 1), NonZeroU64::new(3).unwrap()),
            (("sesn_marker_b".to_owned(), 2), NonZeroU64::new(4).unwrap()),
        ]);
        let batch = runtime_started_marker_audit_batches(&expectations)
            .unwrap()
            .pop()
            .unwrap();
        let first = marker_document("sesn_marker_a", 1, 3);
        let second = marker_document("sesn_marker_b", 2, 4);
        validate_runtime_started_marker_batch(&batch, vec![second.clone(), first.clone()])
            .expect("order is irrelevant when every expected marker is returned");

        let missing = validate_runtime_started_marker_batch(&batch, vec![first.clone()])
            .expect_err("a missing expected position must fail");
        assert!(missing.to_string().contains("missing runtime-start marker"));

        let duplicate =
            validate_runtime_started_marker_batch(&batch, vec![first.clone(), first.clone()])
                .expect_err("a duplicate returned position must fail");
        assert!(duplicate.to_string().contains("duplicate event position"));

        let unexpected = validate_runtime_started_marker_batch(
            &batch,
            vec![marker_document("sesn_marker_c", 3, 5)],
        )
        .expect_err("an unexpected returned position must fail");
        assert!(unexpected.to_string().contains("unexpected event position"));

        let wrong_generation = validate_runtime_started_marker_batch(
            &batch,
            vec![marker_document("sesn_marker_a", 1, 9), second.clone()],
        )
        .expect_err("a marker generation mismatch must fail");
        assert!(wrong_generation.to_string().contains("disagrees"));

        let wrong_kind = super::super::event_doc::to_document(&Event {
            session_id: layer2_core::SessionId("sesn_marker_a".into()),
            seq: 1,
            kind: EventKind::SessionCreated,
            appended_at: "2027-01-15T08:00:00Z".into(),
        })
        .unwrap();
        let wrong_kind = validate_runtime_started_marker_batch(&batch, vec![wrong_kind, second])
            .expect_err("a non-marker at the expected position must fail");
        assert!(wrong_kind
            .to_string()
            .contains("is not a runtime-start marker"));
    }

    #[test]
    fn mongo_completed_switch_and_retirement_receipts_replay_exact_outcomes() {
        let mut session = session_doc();
        let mut receipts = Vec::new();
        let original_request = request(&session, "lease-replay");
        let mut lease = match run(
            &mut session,
            &mut receipts,
            CoordinationCommand::Acquire(original_request),
        )
        .unwrap()
        {
            CoordinationOutcome::Lease(lease) => lease,
            _ => panic!("lease expected"),
        };
        let reservation = match run(
            &mut session,
            &mut receipts,
            CoordinationCommand::Reserve {
                lease: lease.clone(),
                token: "candidate-replay".into(),
            },
        )
        .unwrap()
        {
            CoordinationOutcome::Reservation(reservation) => reservation,
            _ => panic!("reservation expected"),
        };
        let candidate = match run(
            &mut session,
            &mut receipts,
            CoordinationCommand::Register {
                lease: lease.clone(),
                reservation,
                runtime_id: "runtime-replay".into(),
            },
        )
        .unwrap()
        {
            CoordinationOutcome::Candidate(candidate) => candidate,
            _ => panic!("candidate expected"),
        };
        let before_rejected_switch = session.clone();
        let before_rejected_receipts = receipts.clone();
        let mut changed_candidate = candidate.clone();
        changed_candidate.runtime_id = "runtime-replay-changed".into();
        assert!(matches!(
            run_with_runtime_started(
                &mut session,
                &mut receipts,
                CoordinationCommand::Switch {
                    lease: lease.clone(),
                    candidate: changed_candidate,
                },
            ),
            Err(StoreError::RuntimeCandidateChanged { .. })
        ));
        assert_eq!(
            session_bson(&session),
            session_bson(&before_rejected_switch)
        );
        assert_eq!(receipts, before_rejected_receipts);
        let original_lease = lease.clone();
        let existing_sequence_floor = 9;
        let (first_switch_outcome, runtime_started) = run_with_runtime_started_at_sequence_floor(
            &mut session,
            &mut receipts,
            CoordinationCommand::Switch {
                lease: lease.clone(),
                candidate: candidate.clone(),
            },
            existing_sequence_floor,
        )
        .unwrap();
        let first_switch = match first_switch_outcome {
            CoordinationOutcome::Switch(switched) => switched,
            _ => panic!("switch expected"),
        };
        let runtime_started = runtime_started.expect("a first switch emits its marker");
        assert_eq!(runtime_started.seq, existing_sequence_floor + 1);
        assert_eq!(runtime_started.seq, first_switch.runtime_started_seq);
        assert_eq!(session.last_seq, runtime_started.seq as i64);
        assert_eq!(
            runtime_started.appended_at,
            layer2_core::rfc3339_at(DateTime::from_millis(NOW).to_system_time())
        );
        assert!(matches!(
            &runtime_started.kind,
            EventKind::RuntimeStarted { generation }
                if *generation == candidate.generation
        ));
        let receipt_map = receipts
            .iter()
            .cloned()
            .map(|receipt| (receipt.id.clone(), receipt))
            .collect::<BTreeMap<_, _>>();
        let expected_markers = runtime_started_marker_expectations(&receipt_map).unwrap();
        assert_eq!(
            expected_markers.get(&(runtime_started.session_id.0.clone(), runtime_started.seq)),
            Some(&candidate.generation)
        );
        assert_eq!(
            runtime_started_marker_filter(&runtime_started.session_id.0, runtime_started.seq)
                .unwrap(),
            doc! {
                "session_id": &runtime_started.session_id.0,
                "seq": runtime_started.seq as i64,
            }
        );
        let marker_document =
            crate::store::event_doc::to_document(&runtime_started).expect("the marker stores");
        validate_runtime_started_marker(
            &runtime_started.session_id.0,
            runtime_started.seq,
            candidate.generation,
            Some(marker_document),
        )
        .unwrap();
        assert!(validate_runtime_started_marker(
            &runtime_started.session_id.0,
            runtime_started.seq,
            candidate.generation,
            None,
        )
        .is_err());
        let mut legacy_receipt_map = receipt_map.clone();
        let legacy_receipt = legacy_receipt_map
            .values_mut()
            .find(|receipt| {
                matches!(
                    &receipt.payload,
                    CoordinationReceiptPayloadDoc::Reservation(_)
                )
            })
            .expect("the completed reservation receipt exists");
        legacy_receipt.coordination_protocol = super::super::LEGACY_COORDINATION_RECEIPT_PROTOCOL;
        let CoordinationReceiptPayloadDoc::Reservation(legacy_reservation) =
            &mut legacy_receipt.payload
        else {
            unreachable!("the receipt was selected by reservation payload")
        };
        legacy_reservation
            .switch
            .as_mut()
            .expect("the reservation switch completed")
            .runtime_started_seq = 0;
        assert!(legacy_receipt.validate_stored().is_err());
        let mut legacy_document =
            mongodb::bson::serialize_to_document(legacy_receipt).expect("the receipt serializes");
        legacy_document
            .get_document_mut("payload")
            .unwrap()
            .get_document_mut("value")
            .unwrap()
            .get_document_mut("switch")
            .unwrap()
            .remove("runtime_started_seq");
        let legacy_error = coordination_receipt_from_document(legacy_document.clone())
            .expect_err("protocol-1 receipts are not valid in a protocol-3 store");
        assert!(legacy_error
            .to_string()
            .contains("identity or protocol is invalid"));
        legacy_document.insert(
            super::super::COORDINATION_PROTOCOL_FIELD,
            super::super::COORDINATION_RECEIPT_PROTOCOL,
        );
        assert!(coordination_receipt_from_document(legacy_document).is_err());
        let (replayed_switch_outcome, replayed_marker) = run_with_runtime_started(
            &mut session,
            &mut receipts,
            CoordinationCommand::Switch {
                lease: original_lease,
                candidate,
            },
        )
        .unwrap();
        let replayed_switch = match replayed_switch_outcome {
            CoordinationOutcome::Switch(switched) => switched,
            _ => panic!("switch replay expected"),
        };
        assert!(
            replayed_marker.is_none(),
            "a receipt replay cannot emit a second marker"
        );
        assert_eq!(replayed_switch, first_switch);
        lease.boundary = first_switch.boundary;

        let original_retirement_lease = lease.clone();
        let first_retirement = match run(
            &mut session,
            &mut receipts,
            CoordinationCommand::Retire {
                lease: lease.clone(),
                token: "retirement-replay".into(),
            },
        )
        .unwrap()
        {
            CoordinationOutcome::Retirement(retired) => retired,
            _ => panic!("retirement expected"),
        };
        let replayed_retirement = match run(
            &mut session,
            &mut receipts,
            CoordinationCommand::Retire {
                lease: original_retirement_lease,
                token: "retirement-replay".into(),
            },
        )
        .unwrap()
        {
            CoordinationOutcome::Retirement(retired) => retired,
            _ => panic!("retirement replay expected"),
        };
        assert_eq!(replayed_retirement, first_retirement);
    }

    #[test]
    fn released_or_expired_mongo_lease_tokens_never_reacquire_authority() {
        let mut session = session_doc();
        let mut receipts = Vec::new();
        let spent_request = request(&session, "lease-spent");
        let lease = match run(
            &mut session,
            &mut receipts,
            CoordinationCommand::Acquire(spent_request.clone()),
        )
        .unwrap()
        {
            CoordinationOutcome::Lease(lease) => lease,
            _ => panic!("lease expected"),
        };
        run(
            &mut session,
            &mut receipts,
            CoordinationCommand::Release(lease.clone()),
        )
        .unwrap();
        run(
            &mut session,
            &mut receipts,
            CoordinationCommand::Release(lease),
        )
        .expect("a committed release replays from its exact receipt");
        assert!(matches!(
            run(
                &mut session,
                &mut receipts,
                CoordinationCommand::Acquire(spent_request)
            ),
            Err(StoreError::SessionLeaseLost { .. })
        ));

        let expiring = request(&session, "lease-expiring");
        let expiring_lease = match run(
            &mut session,
            &mut receipts,
            CoordinationCommand::Acquire(expiring.clone()),
        )
        .unwrap()
        {
            CoordinationOutcome::Lease(lease) => lease,
            _ => panic!("lease expected"),
        };
        session.coordination.as_mut().unwrap().server_now =
            Some(DateTime::from_millis(NOW + 31_000));
        assert!(matches!(
            apply_command(
                &mut session,
                &CoordinationCommand::Release(expiring_lease),
                &receipts,
            ),
            Err(StoreError::SessionLeaseLost { .. })
        ));
        assert!(matches!(
            apply_command(
                &mut session,
                &CoordinationCommand::Acquire(expiring),
                &receipts,
            ),
            Err(StoreError::SessionLeaseLost { .. })
        ));
    }

    #[test]
    fn bounded_interaction_acquire_and_release_replay_unknown_results() {
        let mut session = session_doc();
        let mut receipts = Vec::new();
        let request = interaction_request(&session, "interaction-nonce-replay");
        let command = CoordinationCommand::AcquireInteraction(request);
        let first = match run(&mut session, &mut receipts, command.clone()).unwrap() {
            CoordinationOutcome::Lease(lease) => lease,
            _ => panic!("interaction lease expected"),
        };
        let replay = match run(&mut session, &mut receipts, command).unwrap() {
            CoordinationOutcome::Lease(lease) => lease,
            _ => panic!("replayed interaction lease expected"),
        };
        assert_eq!(replay, first);
        assert_eq!(session.coordination.as_ref().unwrap().interaction_epoch, 1);
        assert!(receipts.is_empty());
        assert_eq!(session.coordination.as_ref().unwrap().receipt_count, 0);

        let release = CoordinationCommand::Release(first.clone());
        run(&mut session, &mut receipts, release.clone()).unwrap();
        run(&mut session, &mut receipts, release.clone())
            .expect("an unknown successful interaction release replays");

        let successor_request = interaction_request(&session, "interaction-nonce-successor");
        let successor = match run(
            &mut session,
            &mut receipts,
            CoordinationCommand::AcquireInteraction(successor_request),
        )
        .unwrap()
        {
            CoordinationOutcome::Lease(lease) => lease,
            _ => panic!("successor interaction lease expected"),
        };
        run(&mut session, &mut receipts, release)
            .expect("replaying an old release cannot disturb its successor");
        assert_eq!(
            session
                .coordination
                .as_ref()
                .and_then(|state| state.lease.as_ref())
                .unwrap()
                .to_core(&successor.session_id)
                .unwrap(),
            successor
        );
        run(
            &mut session,
            &mut receipts,
            CoordinationCommand::Release(successor),
        )
        .unwrap();
        run(
            &mut session,
            &mut receipts,
            CoordinationCommand::Release(first),
        )
        .expect("an old unknown-success release replays after its successor also released");
    }

    #[test]
    fn bounded_interactions_do_not_grow_receipt_state() {
        let mut session = session_doc();
        let mut receipts = Vec::new();
        for index in 0..512 {
            let request = interaction_request(&session, format!("interaction-nonce-{index}"));
            let lease = match run(
                &mut session,
                &mut receipts,
                CoordinationCommand::AcquireInteraction(request),
            )
            .unwrap()
            {
                CoordinationOutcome::Lease(lease) => lease,
                _ => panic!("interaction lease expected"),
            };
            run(
                &mut session,
                &mut receipts,
                CoordinationCommand::Release(lease),
            )
            .unwrap();
        }

        let state = session.coordination.as_ref().unwrap();
        assert_eq!(state.interaction_epoch, 512);
        assert!(state.lease.is_none());
        assert_eq!(state.receipt_count, 0);
        assert!(receipts.is_empty());
        validate_test_receipts(&session, &receipts).unwrap();
    }

    #[test]
    fn expired_interaction_release_is_a_no_op_beside_a_provider_lease() {
        let mut session = session_doc();
        let mut receipts = Vec::new();
        let interaction = interaction_request(&session, "interaction-nonce-expired-release");
        let expired = match run(
            &mut session,
            &mut receipts,
            CoordinationCommand::AcquireInteraction(interaction),
        )
        .unwrap()
        {
            CoordinationOutcome::Lease(lease) => lease,
            _ => panic!("interaction lease expected"),
        };

        session.coordination.as_mut().unwrap().server_now =
            Some(DateTime::from_millis(NOW + 31_000));
        run(
            &mut session,
            &mut receipts,
            CoordinationCommand::Release(expired.clone()),
        )
        .expect("releasing the expired current interaction succeeds");
        assert!(session.coordination.as_ref().unwrap().lease.is_none());

        let provider_request = request(&session, "provider-after-interaction");
        let provider = match run(
            &mut session,
            &mut receipts,
            CoordinationCommand::Acquire(provider_request),
        )
        .unwrap()
        {
            CoordinationOutcome::Lease(lease) => lease,
            _ => panic!("provider lease expected"),
        };
        run(
            &mut session,
            &mut receipts,
            CoordinationCommand::Release(expired),
        )
        .expect("an old interaction release cannot disturb a provider lease");
        assert_eq!(
            session
                .coordination
                .as_ref()
                .and_then(|state| state.lease.as_ref())
                .unwrap()
                .to_core(&provider.session_id)
                .unwrap(),
            provider
        );
    }

    #[test]
    fn forged_identity_at_the_current_interaction_epoch_cannot_release_it() {
        let mut session = session_doc();
        let mut receipts = Vec::new();
        let request = interaction_request(&session, "interaction-nonce-forged-release");
        let lease = match run(
            &mut session,
            &mut receipts,
            CoordinationCommand::AcquireInteraction(request),
        )
        .unwrap()
        {
            CoordinationOutcome::Lease(lease) => lease,
            _ => panic!("interaction lease expected"),
        };
        let mut forged = lease.clone();
        forged.holder = "forged-holder".into();

        assert!(matches!(
            run(
                &mut session,
                &mut receipts,
                CoordinationCommand::Release(forged),
            ),
            Err(StoreError::SessionLeaseLost { .. })
        ));
        assert_eq!(
            session
                .coordination
                .as_ref()
                .and_then(|state| state.lease.as_ref())
                .unwrap()
                .to_core(&lease.session_id)
                .unwrap(),
            lease
        );
    }

    #[test]
    fn malformed_interaction_release_token_is_reported_as_lease_loss() {
        let mut session = session_doc();
        let mut receipts = Vec::new();
        let request = interaction_request(&session, "interaction-nonce-malformed-release");
        let lease = match run(
            &mut session,
            &mut receipts,
            CoordinationCommand::AcquireInteraction(request),
        )
        .unwrap()
        {
            CoordinationOutcome::Lease(lease) => lease,
            _ => panic!("interaction lease expected"),
        };
        let mut malformed = lease.clone();
        malformed.token = "not-an-interaction-epoch".into();

        assert!(matches!(
            run(
                &mut session,
                &mut receipts,
                CoordinationCommand::Release(malformed),
            ),
            Err(StoreError::SessionLeaseLost { .. })
        ));
        assert_eq!(
            session
                .coordination
                .as_ref()
                .and_then(|state| state.lease.as_ref())
                .unwrap()
                .to_core(&lease.session_id)
                .unwrap(),
            lease
        );
    }

    #[test]
    fn interaction_renewal_allows_status_drift_but_rejects_runtime_drift() {
        let mut session = session_doc();
        let mut receipts = Vec::new();
        let request = interaction_request(&session, "interaction-nonce-drift");
        let lease = match run(
            &mut session,
            &mut receipts,
            CoordinationCommand::AcquireInteraction(request),
        )
        .unwrap()
        {
            CoordinationOutcome::Lease(lease) => lease,
            _ => panic!("interaction lease expected"),
        };
        session.status = serde_plain_string(&SessionStatus::Idle).unwrap();

        run(
            &mut session,
            &mut receipts,
            CoordinationCommand::RenewInteraction {
                lease: lease.clone(),
                ttl: std::time::Duration::from_secs(30),
            },
        )
        .expect("an interaction may renew across its own status transition");
        assert!(matches!(
            run(
                &mut session,
                &mut receipts,
                CoordinationCommand::Renew {
                    lease: lease.clone(),
                    ttl: std::time::Duration::from_secs(30),
                },
            ),
            Err(StoreError::Malformed(message)) if message.contains("interaction renewal")
        ));

        session.runtime_id = Some("runtime-successor".into());
        assert!(matches!(
            run(
                &mut session,
                &mut receipts,
                CoordinationCommand::RenewInteraction {
                    lease: lease.clone(),
                    ttl: std::time::Duration::from_secs(30),
                },
            ),
            Err(StoreError::SessionLeaseLost { .. })
        ));
        run(
            &mut session,
            &mut receipts,
            CoordinationCommand::Release(lease),
        )
        .expect("the exact bounded token can be cleared after status drift");
    }

    #[test]
    fn interaction_renewal_rejects_runtime_generation_drift() {
        let mut session = session_doc();
        let mut receipts = Vec::new();
        let request = interaction_request(&session, "interaction-nonce-generation-drift");
        let lease = match run(
            &mut session,
            &mut receipts,
            CoordinationCommand::AcquireInteraction(request),
        )
        .unwrap()
        {
            CoordinationOutcome::Lease(lease) => lease,
            _ => panic!("interaction lease expected"),
        };
        let state = session.coordination.as_mut().unwrap();
        state.authority_generation += 1;
        state.next_generation = state.authority_generation;

        assert!(matches!(
            run(
                &mut session,
                &mut receipts,
                CoordinationCommand::RenewInteraction {
                    lease,
                    ttl: std::time::Duration::from_secs(30),
                },
            ),
            Err(StoreError::SessionLeaseLost { .. })
        ));
    }

    #[test]
    fn provider_renewal_still_requires_status_to_match() {
        let mut session = session_doc();
        let mut receipts = Vec::new();
        let request = request(&session, "provider-status-renewal");
        let lease = match run(
            &mut session,
            &mut receipts,
            CoordinationCommand::Acquire(request),
        )
        .unwrap()
        {
            CoordinationOutcome::Lease(lease) => lease,
            _ => panic!("provider lease expected"),
        };
        session.status = serde_plain_string(&SessionStatus::Idle).unwrap();

        assert!(matches!(
            run(
                &mut session,
                &mut receipts,
                CoordinationCommand::Renew {
                    lease,
                    ttl: std::time::Duration::from_secs(30),
                },
            ),
            Err(StoreError::SessionLeaseLost { .. })
        ));
    }

    #[test]
    fn interaction_nonce_is_validated_at_the_mongo_transition_boundary() {
        let mut session = session_doc();
        let mut receipts = Vec::new();
        let mut request = interaction_request(&session, "interaction-nonce-invalid");
        request.nonce.clear();

        assert!(matches!(
            run(
                &mut session,
                &mut receipts,
                CoordinationCommand::AcquireInteraction(request),
            ),
            Err(StoreError::Malformed(message)) if message.contains("nonce")
        ));
    }

    #[test]
    fn expired_interaction_replay_gets_a_new_epoch_and_cannot_clear_its_successor() {
        let mut session = session_doc();
        let mut receipts = Vec::new();
        let request = interaction_request(&session, "interaction-nonce-expiry");
        let old = match run(
            &mut session,
            &mut receipts,
            CoordinationCommand::AcquireInteraction(request.clone()),
        )
        .unwrap()
        {
            CoordinationOutcome::Lease(lease) => lease,
            _ => panic!("interaction lease expected"),
        };
        assert!(has_session_lease(&session).unwrap());

        session.coordination.as_mut().unwrap().server_now =
            Some(DateTime::from_millis(NOW + 31_000));
        assert!(!has_session_lease(&session).unwrap());
        let (outcome, mutations, runtime_started) = apply_command(
            &mut session,
            &CoordinationCommand::AcquireInteraction(request),
            &receipts,
        )
        .unwrap();
        assert!(runtime_started.is_none());
        apply_mutations(&mut receipts, mutations);
        let successor = match outcome {
            CoordinationOutcome::Lease(lease) => lease,
            _ => panic!("successor interaction lease expected"),
        };
        assert_ne!(old.token, successor.token);
        assert_eq!(session.coordination.as_ref().unwrap().interaction_epoch, 2);
        assert!(has_session_lease(&session).unwrap());

        assert!(matches!(
            apply_command(
                &mut session,
                &CoordinationCommand::RenewInteraction {
                    lease: old.clone(),
                    ttl: std::time::Duration::from_secs(30),
                },
                &receipts,
            ),
            Err(StoreError::SessionLeaseLost { .. })
        ));
        apply_command(&mut session, &CoordinationCommand::Release(old), &receipts)
            .expect("an older interaction release is a no-op beside its successor");
        assert_eq!(
            session
                .coordination
                .as_ref()
                .and_then(|state| state.lease.as_ref())
                .unwrap()
                .to_core(&successor.session_id)
                .unwrap(),
            successor
        );
        assert!(receipts.is_empty());
        assert_eq!(session.coordination.as_ref().unwrap().receipt_count, 0);
    }

    #[test]
    fn mongo_receipt_audit_detects_missing_permanent_and_cleanup_receipts() {
        let mut session = session_doc();
        let mut receipts = Vec::new();
        let acquire = request(&session, "lease-audit");
        let lease = match run(
            &mut session,
            &mut receipts,
            CoordinationCommand::Acquire(acquire),
        )
        .unwrap()
        {
            CoordinationOutcome::Lease(lease) => lease,
            _ => panic!("lease expected"),
        };
        validate_test_receipts(&session, &receipts).unwrap();
        let lease_receipt = receipts.remove(0);
        assert!(validate_test_receipts(&session, &receipts).is_err());
        receipts.push(lease_receipt);

        let reservation = match run(
            &mut session,
            &mut receipts,
            CoordinationCommand::Reserve {
                lease: lease.clone(),
                token: "candidate-audit".into(),
            },
        )
        .unwrap()
        {
            CoordinationOutcome::Reservation(reservation) => reservation,
            _ => panic!("reservation expected"),
        };
        run(
            &mut session,
            &mut receipts,
            CoordinationCommand::Register {
                lease,
                reservation,
                runtime_id: "runtime-audit".into(),
            },
        )
        .unwrap();
        validate_test_receipts(&session, &receipts).unwrap();
        receipts.retain(|receipt| receipt.id.kind != RECEIPT_CLEANUP);
        assert!(validate_test_receipts(&session, &receipts).is_err());
    }

    #[test]
    fn completed_cleanup_acknowledgement_replays_under_a_successor_lease() {
        let mut session = session_doc();
        let mut receipts = Vec::new();
        let acquire = request(&session, "lease-cleanup-first");
        let mut lease = match run(
            &mut session,
            &mut receipts,
            CoordinationCommand::Acquire(acquire),
        )
        .unwrap()
        {
            CoordinationOutcome::Lease(lease) => lease,
            _ => panic!("lease expected"),
        };
        let retired = match run(
            &mut session,
            &mut receipts,
            CoordinationCommand::Retire {
                lease: lease.clone(),
                token: "cleanup-successor".into(),
            },
        )
        .unwrap()
        {
            CoordinationOutcome::Retirement(retired) => retired,
            _ => panic!("retirement expected"),
        };
        lease.boundary = retired.boundary;
        run(
            &mut session,
            &mut receipts,
            CoordinationCommand::Acknowledge {
                lease: lease.clone(),
                cleanup: retired.cleanup.clone(),
            },
        )
        .unwrap();
        run(
            &mut session,
            &mut receipts,
            CoordinationCommand::Release(lease),
        )
        .unwrap();
        let acquire_successor = request(&session, "lease-cleanup-successor");
        let successor = match run(
            &mut session,
            &mut receipts,
            CoordinationCommand::Acquire(acquire_successor),
        )
        .unwrap()
        {
            CoordinationOutcome::Lease(lease) => lease,
            _ => panic!("successor lease expected"),
        };
        run(
            &mut session,
            &mut receipts,
            CoordinationCommand::Acknowledge {
                lease: successor,
                cleanup: retired.cleanup,
            },
        )
        .expect("an exact completed cleanup replays under any current authorized lease");
        validate_test_receipts(&session, &receipts).unwrap();
    }

    #[test]
    fn cleanup_receipt_bson_shape_supports_the_active_debt_query() {
        let id = SessionId("sesn_bson_cleanup".into());
        let cleanup = RuntimeCleanup {
            session_id: id.clone(),
            generation: 9,
            runtime_id: "runtime-nine".into(),
            token: "retired:nine".into(),
            reason: RuntimeCleanupReason::RetiredAuthority,
        };
        let stored = cleanup_receipt(&id, &cleanup).unwrap();
        let mut document = mongodb::bson::serialize_to_document(&stored).unwrap();
        assert_eq!(
            document
                .get_document("payload")
                .unwrap()
                .get_str("type")
                .unwrap(),
            "cleanup"
        );
        assert!(!document
            .get_document("payload")
            .unwrap()
            .get_document("value")
            .unwrap()
            .contains_key("completed_by"));
        assert_eq!(cleanup_from_receipt(&stored).unwrap(), cleanup);
        coordination_receipt_from_document(document.clone())
            .expect("the canonical active cleanup BSON shape decodes");
        document
            .get_document_mut("payload")
            .unwrap()
            .get_document_mut("value")
            .unwrap()
            .insert("completed_by", Bson::Null);
        assert!(coordination_receipt_from_document(document).is_err());
        assert_eq!(
            active_cleanup_filter(&id),
            doc! {
                "_id.session_id": "sesn_bson_cleanup",
                "_id.kind": "cleanup",
                "payload.type": "cleanup",
                "$or": [
                    { "payload.value.completed_by": { "$exists": false } },
                    { "payload.value.completed_by": Bson::Null },
                ],
            }
        );
    }

    #[test]
    fn coordination_clock_promotes_a_pre_protocol_session_atomically() {
        assert_eq!(
            coordination_clock_update(),
            doc! {
                "$set": {
                    super::super::WRITE_PROTOCOL_FIELD: super::super::SESSION_WRITE_PROTOCOL,
                    super::super::COORDINATION_PROTOCOL_FIELD:
                        super::super::SESSION_COORDINATION_PROTOCOL,
                },
                "$max": { "revision": 0_i64 },
                "$currentDate": { "coordination.server_now": true },
            }
        );
    }
}
