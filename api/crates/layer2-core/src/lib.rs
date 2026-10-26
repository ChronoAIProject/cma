//! Protocol-independent foundations retained during the CMA domain rebuild.
//!
//! The resource aggregates are introduced by issue #802. Until then this
//! crate deliberately exposes only identity, authorization, audit-detail,
//! idempotency, time, and pressure primitives that do not presume a resource
//! hierarchy.

pub mod activity;
pub mod activity_detail;
pub mod activity_service;
pub mod authz;
pub mod bot_setups;
pub mod bots;
pub mod capabilities;
pub mod capacity_bounds;
pub mod clock;
pub mod deployment_capacity;
pub mod directory;
pub mod document_artifacts;
pub mod error;
pub mod event;
mod event_kinds;
pub mod gateway_interactions;
pub mod gateway_records;
pub mod ids;
pub mod inventory;
pub mod limits;
pub mod log;
pub mod operation;
pub mod paging;
pub mod platform;
pub mod platform_service;
pub mod pressure;
pub mod pressure_service;
pub mod profiles;
pub mod resource_access;
pub mod sandbox;
pub mod sequence;
pub mod trigger_domain;

pub use activity::{
    ActivityAction, ActivityId, ActivityOutcome, ActivityQuery, ActivityRecord, ActivityTarget,
    DEFAULT_ACTIVITY_LIMIT, MAX_ACTIVITY_LIMIT,
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
    SkillId, SubjectId, WorkspaceId, WorkspaceProfileId, WorktreeId,
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
    PlatformSettingsService, PlatformSettingsStore, StoredPlatformSettings,
};
pub use pressure::{
    CpuPressure, CpuPressurePolicy, CpuPressureSample, CpuPressureTransition,
    InvalidPressurePolicy, MemoryPressure, MemoryPressurePolicy, PressureSample,
    PressureTransition,
};
pub use pressure_service::{PressureEdges, PressureMonitor};
pub use sequence::Sequences;

pub mod event_workers;
