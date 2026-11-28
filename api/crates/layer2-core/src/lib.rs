//! Protocol-independent foundations retained during the CMA domain rebuild.
//!
//! The resource aggregates are introduced by issue #802. Until then this
//! crate deliberately exposes only identity, authorization, audit-detail,
//! idempotency, time, and pressure primitives that do not presume a resource
//! hierarchy.

pub mod account_deletion;
pub mod activity;
pub mod activity_detail;
pub mod activity_service;
pub mod admission;
pub mod agent_ownership;
pub mod authz;
pub mod billing_authority;
pub mod binding_diagnostics;
pub mod bot_setups;
pub mod bots;
pub mod budget;
pub mod capabilities;
pub mod capacity_bounds;
pub mod clock;
pub mod deployment_capacity;
pub mod diagnostic_context;
pub mod directory;
pub mod document_artifacts;
pub mod error;
pub mod event;
mod event_kinds;
pub mod gateway_interactions;
pub mod gateway_records;
pub mod ids;
pub mod inventory;
pub mod legal_consent;
pub mod limits;
pub mod log;
pub mod operation;
pub mod paging;
pub mod platform;
pub mod platform_service;
pub mod pressure;
pub mod pressure_service;
pub mod profiles;
#[cfg(feature = "qualification-hooks")]
pub mod qualification;
pub mod released_runtime;
pub mod resource_access;
mod retry_schedule;
pub mod sandbox;
pub mod sandbox_metrics;
pub mod sandbox_usage;
pub mod sandbox_usage_cursor;
pub mod sandbox_usage_evidence;
pub mod sandbox_usage_service;
pub mod sequence;
pub mod trigger_domain;

pub use activity::{
    ActivityAction, ActivityId, ActivityOutcome, ActivityQuery, ActivityRecord, ActivitySummary,
    ActivityTarget, DEFAULT_ACTIVITY_LIMIT, MAX_ACTIVITY_LIMIT,
};
pub use activity_detail::{ActivityDetail, DetailKey};
pub use activity_service::{ActivityService, ActivityStore};
pub use authz::{Authorization, Caller, Capability};
pub use capabilities::{RuntimeCapabilities, RuntimeCapability};
pub use capacity_bounds::{CapacityBounds, CapacityDimension, OutOfBounds, RequestedCapacity};
pub use clock::{now_rfc3339, rfc3339_at, seconds_from_rfc3339};
pub use deployment_capacity::{AvailableCapacity, DeploymentCapacity, ReservedCapacity};
pub use directory::{
    DirectoryQuery, PublicIdentity, RegisteredUser, UserDirectory, UserDirectoryStore,
};
pub use error::CoreError;
pub use event::{Event, EventKind, PressureDimension};
pub use gateway_records::{
    GatewayRecord, GatewayRecordHead, GatewayRecordService, GatewayRecordStore,
    MAX_GATEWAY_RECORD_BLOB_BYTES, MAX_GATEWAY_RECORD_PAGE_SIZE, MIN_GATEWAY_RECORD_BLOB_BYTES,
};
pub use ids::{
    AgentId, AgentProfileId, LaunchId, SandboxId, SandboxProfileId, ServiceHandoffId, ServiceId,
    SkillId, SubjectId, WorkspaceId, WorkspaceProfileId, WorktreeId, DELETED_ACCOUNT_SUBJECT,
};
pub use inventory::{ProviderInventory, RuntimeEntry, RuntimeInventory, RuntimeStatus};
pub use limits::CountLimit;
pub use log::{EventLog, EventStore};
pub use operation::{OperationClaim, OperationReservation, OperationStore, StoreError};
pub use paging::{Page, PageRequest, DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE};
pub use platform::{
    CheckpointSettings, InactivitySettings, MarketplaceSettings, PlatformDefaults, PlatformLimits,
    PlatformSettings, RetentionSettings, TriggerSettings, DEFAULT_AGENT_PROFILE_ID,
    DEFAULT_CHECKPOINT_INTERVAL_SECONDS, DEFAULT_PROFILE_REVISION, DEFAULT_SANDBOX_PROFILE_ID,
    DEFAULT_WORKSPACE_PROFILE_ID, MAX_PAUSE_AFTER_SECONDS, MIN_PAUSE_AFTER_SECONDS,
};
pub use platform_service::{
    PlatformSettingChange, PlatformSettingChanges, PlatformSettingsService, PlatformSettingsStore,
    StoredPlatformSettings,
};
pub use pressure::{
    CpuPressure, CpuPressurePolicy, CpuPressureSample, CpuPressureTransition,
    InvalidPressurePolicy, MemoryPressure, MemoryPressurePolicy, PressureSample,
    PressureTransition,
};
pub use pressure_service::{PressureEdges, PressureMonitor};
pub use sequence::Sequences;

pub mod dependency_diagnostics;
pub mod event_workers;
