//! Preserve typed service refusals over the authenticated private hop.
use layer2_core::{
    budget::BudgetError,
    resource_access::{
        AccessError, MembershipError, ResourceAccessError, ResourceAccessStoreError,
    },
    sandbox::{
        agent_io::*, AgentOperationError, AgentStatus, FirstMessageDelivery,
        SandboxCredentialPrerequisiteError,
    },
    RuntimeCapability, ServiceId,
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub(super) struct AdmissionFailure(#[serde(with = "Admission")] pub CommandAdmissionError);
#[derive(Serialize, Deserialize)]
pub(super) struct ControlFailure(#[serde(with = "Control")] pub TargetedControlError);
#[derive(Serialize, Deserialize)]
pub(super) struct QueueFailure(#[serde(with = "Queue")] pub QueuedCancellationError);

#[derive(Serialize, Deserialize)]
#[serde(remote = "CommandAdmissionError")]
enum Admission {
    Access(#[serde(with = "Resource")] ResourceAccessError),
    Authority(#[serde(with = "Authority")] CommandAuthorizationError),
    Ledger(#[serde(with = "Ledger")] CommandLedgerError),
    Input(#[serde(with = "Input")] AgentOperationError),
    ModelMismatch,
}
#[derive(Serialize, Deserialize)]
#[serde(remote = "TargetedControlError")]
enum Control {
    Admission(#[serde(with = "Admission")] CommandAdmissionError),
    CapabilityUnsupported,
    NotAdmitted,
}
#[derive(Serialize, Deserialize)]
#[serde(remote = "QueuedCancellationError")]
enum Queue {
    Admission(#[serde(with = "Admission")] CommandAdmissionError),
    NotAdmitted,
}
#[derive(Serialize, Deserialize)]
#[serde(remote = "ResourceAccessError")]
enum Resource {
    Access(#[serde(with = "Access")] AccessError),
    Store(#[serde(with = "Store")] ResourceAccessStoreError),
    Membership(#[serde(with = "Membership")] MembershipError),
}
#[derive(Serialize, Deserialize)]
#[serde(remote = "AccessError")]
enum Access {
    NotFound,
    Forbidden,
    RevalidationRequired,
    UnsafeInteractiveShare,
}
#[derive(Serialize, Deserialize)]
#[serde(remote = "ResourceAccessStoreError")]
enum Store {
    NotFound,
    Conflict,
    Corrupt,
    Unavailable,
    UnsafeInteractiveShare,
    Capacity,
    Invalid,
    IdempotencyConflict,
}
#[derive(Serialize, Deserialize)]
#[serde(remote = "MembershipError")]
enum Membership {
    NotMember,
    InvalidEvidence,
    Unavailable,
}
#[derive(Serialize, Deserialize)]
#[serde(remote = "CommandAuthorizationError")]
enum Authority {
    Revoked,
    Unavailable,
}
#[derive(Serialize, Deserialize)]
#[serde(remote = "CommandLedgerError")]
enum Ledger {
    Budget(BudgetError),
    AdmissionPaused,
    Invalid,
    NotFound,
    IdempotencyConflict,
    Conflict,
    StaleClaim,
    NotAdmissible,
    QueueFull,
    Corrupt,
    Unavailable,
}
#[derive(Serialize, Deserialize)]
#[serde(remote = "SandboxCredentialPrerequisiteError")]
enum Connection {
    SetupRequired,
    ConnectionsRequired { services: Vec<ServiceId> },
    Unavailable(String),
}
#[derive(Serialize, Deserialize)]
#[serde(remote = "AgentOperationError")]
enum Input {
    Budget(BudgetError),
    AdmissionPaused,
    Connection(#[serde(with = "Connection")] SandboxCredentialPrerequisiteError),
    Forbidden,
    NotFound,
    StatusConflict {
        status: AgentStatus,
    },
    NoActiveTurn,
    ApprovalNotPending {
        approval_id: String,
    },
    FirstMessageReserved {
        state: FirstMessageDelivery,
    },
    DispatchInProgress {
        operation_sequence: u64,
    },
    DispatchRecoveryRequired {
        operation_sequence: u64,
    },
    CapabilityUnsupported {
        capability: RuntimeCapability,
    },
    ContentUnsupported,
    QueueCapacityExceeded {
        limit: usize,
    },
    StaleWorkAuthority,
    InvalidRequest(String),
    SkillVersionUpgradeUnsupported {
        id: String,
        installed: String,
        requested: String,
    },
    SkillUpgradeDisabled,
    SkillHistoryFull,
    Conflict,
    CheckpointInProgress,
    PreviousTurnConflict,
    LogCapacityExceeded,
    Corrupt(String),
    RuntimeUnavailable(String),
    DeliveryAmbiguous {
        operation_sequence: u64,
        detail: String,
    },
    OutcomeUnavailable(String),
    Unavailable(String),
}
