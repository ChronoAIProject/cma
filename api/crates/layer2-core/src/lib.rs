//! Layer 2 — core session model and events.
//!
//! The single tunnel every Layer 1 interface talks to. Protocol-agnostic and
//! kernel-agnostic: no protocol name and no kernel name may appear here, in
//! identifiers, types, comments, or doc examples.
//!
//! The core is a superset, never an intersection: it models the union of what
//! every interface and every runtime needs, so that an interface exposing a
//! subset is a projection rather than a compromise.

pub mod activity;
pub mod activity_detail;
pub mod activity_service;
pub mod authz;
pub mod capabilities;
mod capacity_bounds;
pub mod chat;
pub mod clock;
pub mod conversation;
mod conversation_log;
mod conversation_opening;
mod coordination;
mod credentials;
pub mod deployment_capacity;
pub mod directory;
pub mod environment;
mod environment_catalog;
pub mod environment_service;
mod environment_values;
pub mod error;
pub mod event;
mod event_kinds;
pub mod ids;
pub mod inventory;
mod lease;
mod lifecycle;
mod limits;
mod log;
mod platform;
pub mod platform_service;
mod pressure;
mod pressure_service;
mod pricing;
pub mod profiles;
mod provision;
mod provision_failure;
mod provision_support;
mod reading;
mod readopt;
pub mod reconcile;
pub mod resources;
mod rotate;
mod runtime_policy;
pub mod sandbox;
mod sequence;
mod spend;
mod sponsored_model_authority;

#[cfg(test)]
#[path = "provision_tests.rs"]
mod provision_tests;

#[cfg(test)]
mod adapter_support;
#[cfg(test)]
mod commit_fault_support;
#[cfg(test)]
mod conversation_coordination_tests;
#[cfg(test)]
mod conversation_readiness_tests;
#[cfg(test)]
mod conversation_support;
#[cfg(test)]
mod environment_support;
#[cfg(test)]
mod screen_support;
#[cfg(test)]
mod service_support;
#[cfg(test)]
mod test_support;

#[cfg(test)]
mod allowance_tests;
#[cfg(test)]
mod approval_recovery_tests;
#[cfg(test)]
mod capacity_bounds_service_tests;
#[cfg(test)]
mod deployment_capacity_tests;
#[cfg(test)]
mod limits_rotation_tests;
#[cfg(test)]
mod limits_tests;
#[cfg(test)]
mod pressure_service_tests;
#[cfg(test)]
mod pressure_settings_tests;
#[cfg(test)]
mod pressure_tests;
#[cfg(test)]
mod pricing_service_tests;
#[cfg(test)]
mod resources_tests;
#[cfg(test)]
mod runtime_interaction_tests;
#[cfg(test)]
mod spend_tests;
#[cfg(test)]
mod trigger_admission_tests;

#[cfg(test)]
#[path = "persistence_tests.rs"]
mod persistence_tests;

pub mod attach;
#[cfg(test)]
#[path = "attach_tests.rs"]
mod attach_tests;
#[cfg(test)]
#[path = "lifecycle_tests.rs"]
mod lifecycle_tests;
pub mod options;
mod recovery_pair;
#[cfg(test)]
#[path = "recovery_tests.rs"]
mod recovery_tests;
#[cfg(test)]
#[path = "rotate_tests.rs"]
mod rotate_tests;
pub mod service;
mod service_state;
pub mod session;
pub mod size;
#[cfg(test)]
#[path = "skill_pack_tests.rs"]
mod skill_pack_tests;
pub mod store;
pub mod terminal;
mod terminal_io;
mod transcript;
#[cfg(test)]
#[path = "transcript_tests.rs"]
mod transcript_tests;
pub mod trigger;
mod trigger_cancellation;
mod trigger_prompt;
mod workspace_checkpoint;
pub use workspace_checkpoint::{TriggerLifecycleCheckpointOutcome, WorkspaceWriterHoldReceipt};

pub use activity::{
    ActivityAction, ActivityId, ActivityOutcome, ActivityQuery, ActivityRecord, ActivityTarget,
    DEFAULT_ACTIVITY_LIMIT, MAX_ACTIVITY_LIMIT,
};
pub use activity_detail::{ActivityDetail, DetailKey};
pub use activity_service::ActivityService;
pub use attach::AttachmentClaim;
pub use authz::{Authorization, Caller, Capability};
pub use capabilities::{servable_modes, Capability as SessionCapability, SessionCapabilities};
pub use chat::{
    ChatAgentSettings, ChatCheckpointConversation, ChatCheckpointConversationRef,
    ChatCheckpointOutcome, ChatCheckpointRef, ChatCheckpointSweep, ChatHostFailure, ChatHostId,
    ChatHostListQuery, ChatHostOperatorNote, ChatHostRecord, ChatHostSettings, ChatHostStatus,
    ChatInactivity, ChatPauseOutcome, ChatPromptAdmission, ChatPromptDeliveryState,
    ChatProvisioningPhase, ChatReviveCause, ChatRevivePhase, ChatSettings, ChatSettingsSnapshot,
    ChatSettingsViolation, ChatSetup, ChatThreadRestoreFailure, ChatThreadSlot,
    ChatTitleGenerationError, ChatTitleGenerationRequest, ChatTitleGenerator, ChatTitleOutcome,
    ChatUnavailableCause, ChatWorkspaceAuthority, ChatWorkspaceSource,
    CHAT_CHECKPOINT_INTERVAL_SECONDS, CHAT_PAUSE_REFUSAL_BACKOFF_SECONDS,
    CHAT_TITLE_CALL_TIMEOUT_SECONDS, CHAT_TITLE_FALLBACK_CHARACTERS, CHAT_TITLE_INPUT_BYTES,
    CHAT_TITLE_MAX_CHARACTERS, CHAT_TITLE_SYSTEM_PROMPT, MAX_CHAT_CONVERSATION_SNAPSHOT_BYTES,
    MAX_CHAT_PROMPT_ADMISSIONS, MAX_CHAT_PROMPT_BYTES,
};
pub use clock::{rfc3339_at, seconds_from_rfc3339};
pub use conversation::{
    ApprovalDecision, ApprovalResolution, ConversationReadinessError, Usage,
    CONVERSATION_ADVANCED_DETAIL, CONVERSATION_IN_PROGRESS_DETAIL, MAX_TURN_METADATA_KEY_CHARS,
    MAX_TURN_METADATA_PAIRS, MAX_TURN_METADATA_VALUE_CHARS,
};
pub use coordination::{
    ChatRuntimeTransition, ConversationLink, ConversationOpeningLease,
    ConversationOpeningLeaseRequest, ConversationWriteAuthority, RuntimeAuthority,
    RuntimeAuthorityRetirement, RuntimeAuthoritySwitch, RuntimeCandidate, RuntimeCleanup,
    RuntimeCleanupReason, RuntimeReservation, SessionBoundary, SessionInteractionLeaseRequest,
    SessionLease, SessionLeaseRequest, SessionOperation,
};
pub use credentials::CredentialDeliveryTarget;
pub use deployment_capacity::{DeploymentCapacity, Reserved};
pub use directory::{
    DirectoryQuery, PublicIdentity, RegisteredUser, UserDirectory, DEFAULT_DIRECTORY_LIMIT,
    MAX_DIRECTORY_LIMIT,
};
pub use environment::{
    AgentConfiguration, Baseline, CatalogError, CatalogService, CatalogSkill, DescribedSkill,
    EnvironmentProfile, EnvironmentProfileSpec, EnvironmentVariable, GrantedSkill,
    ResolvedEnvironment, ServiceAvailability, ServiceCatalog, ServiceId, SkillCatalog, SkillId,
    SoftwarePackage,
};
pub use environment_service::EnvironmentProfileService;
pub use error::CoreError;
pub use event::{Event, EventKind, LiveFrame, StoredEvent, StoredEventKind, ToolBody};
pub use ids::{EnvironmentProfileId, KernelId, SessionId, SubjectId};
pub use inventory::{
    ProviderInventory, ProviderRuntimeGeneration, ProviderView, RuntimeAuthorityComparison,
    RuntimeEntry, RuntimeInventory,
};
pub use layer3_adapter::PermissionLocator;
pub use limits::SessionAllowance;
pub use options::{AgentOptions, CredentialMode, PermissionMode, ReasoningEffort, SessionMode};
pub use platform::{
    accrue, format_usd, Accrual, CapacityBounds, OutOfBounds, PlatformSettings, PriceList,
    PriceListViolation, Quote, Rate, RateCard, RateCardId, SessionBriefing, SessionPricing,
    SizeDimension, CURRENCY,
};
pub use platform_service::PlatformSettingsService;
pub use pressure::{
    CpuPressure, CpuPressurePolicy, CpuPressureSample, CpuPressureTransition,
    InvalidPressurePolicy, MemoryPressure, MemoryPressurePolicy, PressureSample,
    PressureTransition,
};
pub use reconcile::Reclaimed;
pub use recovery_pair::{
    TriggerRecoveryPair, TriggerRecoveryPairCommitReceipt, TriggerRecoveryPairRef,
    MAX_TRIGGER_CONVERSATION_SNAPSHOT_BYTES, TRIGGER_RECOVERY_PAIR_VERSION,
};
pub use resources::{HeldReading, ResourceReading, ResourceStatus};
pub use runtime_policy::{
    RuntimeGatewayReachabilityError, RuntimeGatewayReachabilityReceipt,
    RuntimeGatewayReconciliationReceipt, RuntimePolicyObservation,
};
pub use service::{
    AttachmentHandle, Deployment, Detached, ScreenFactory, SessionPorts, SessionService,
};
pub use session::{
    trigger_session_id, CallerCredential, ChatSession, RecoveryState, RuntimeLossCause, Session,
    SessionCredentialAuthority, SessionSpec, SessionStatus, SessionSurface, StopReason,
    TriggerSessionOrigin,
};
pub use size::{InvalidSessionSize, SessionSize};
pub use spend::Spend;
pub use sponsored_model_authority::SponsoredModelAuthorityError;
pub use store::{
    ActivityStore, ChatCheckpointObjectReceipt, ChatCheckpointStore, ChatOrigin, ContextStore,
    CredentialSource, EnvironmentProfileStore, HandoverSnapshot, HandoverStore,
    InitialPromptDispatch, OperationClaim, OperationReservation, OperationStore, Page, PageCursor,
    PageRequest, PendingHandover, PendingHandoverTarget, SessionListQuery, SessionRecord,
    SessionStore, SessionSurfaceFilter, SessionWorkspaceSource, SkillPack, SkillPackDigest,
    SkillPackState, SkillPackStore, SkillSource, StoreError, UserDirectoryStore, DEFAULT_PAGE_SIZE,
    MAX_PAGE_SIZE,
};
pub use store::{ChatHostStore, PlatformSettingsStore, StoredSettings};
pub use terminal::{InvalidGeometry, ScreenModel, ScreenSnapshot, TerminalGeometry};
pub use trigger::*;
