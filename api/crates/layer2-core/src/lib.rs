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
pub mod clock;
pub mod conversation;
mod conversation_log;
mod credentials;
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
mod provision;
mod provision_failure;
mod reading;
mod readopt;
pub mod reconcile;
mod rotate;
mod sequence;

#[cfg(test)]
#[path = "provision_tests.rs"]
mod provision_tests;

#[cfg(test)]
mod adapter_support;
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
mod capacity_bounds_service_tests;
#[cfg(test)]
mod limits_rotation_tests;
#[cfg(test)]
mod limits_tests;

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
#[cfg(test)]
#[path = "recovery_tests.rs"]
mod recovery_tests;
#[cfg(test)]
#[path = "rotate_tests.rs"]
mod rotate_tests;
pub mod service;
pub mod session;
pub mod size;
pub mod store;
pub mod terminal;
mod terminal_io;
mod transcript;
#[cfg(test)]
#[path = "transcript_tests.rs"]
mod transcript_tests;

pub use activity::{
    ActivityAction, ActivityId, ActivityOutcome, ActivityQuery, ActivityRecord, ActivityTarget,
    DEFAULT_ACTIVITY_LIMIT, MAX_ACTIVITY_LIMIT,
};
pub use activity_detail::{ActivityDetail, DetailKey};
pub use activity_service::ActivityService;
pub use attach::AttachmentClaim;
pub use authz::{Authorization, Caller, Capability};
pub use capabilities::{servable_modes, Capability as SessionCapability, SessionCapabilities};
pub use clock::rfc3339_at;
pub use conversation::{ApprovalDecision, Usage};
pub use directory::{
    DirectoryQuery, PublicIdentity, RegisteredUser, UserDirectory, DEFAULT_DIRECTORY_LIMIT,
    MAX_DIRECTORY_LIMIT,
};
pub use environment::{
    AgentConfiguration, Baseline, CatalogError, CatalogService, CatalogSkill, EnvironmentProfile,
    EnvironmentProfileSpec, EnvironmentVariable, GrantedSkill, ResolvedEnvironment,
    ServiceAvailability, ServiceCatalog, ServiceId, SkillCatalog, SkillId, SoftwarePackage,
};
pub use environment_service::EnvironmentProfileService;
pub use error::CoreError;
pub use event::{Event, EventKind, LiveFrame, StoredEvent, StoredEventKind};
pub use ids::EnvironmentProfileId;
pub use ids::{KernelId, SessionId, SubjectId};
pub use inventory::{ProviderInventory, ProviderView, RuntimeEntry, RuntimeInventory};
pub use options::{AgentOptions, CredentialMode, PermissionMode, ReasoningEffort, SessionMode};
pub use platform::{CapacityBounds, OutOfBounds, PlatformSettings, SessionBriefing, SizeDimension};
pub use platform_service::PlatformSettingsService;
pub use reconcile::Reclaimed;
pub use service::{
    AttachmentHandle, Deployment, Detached, ScreenFactory, SessionPorts, SessionService,
};
pub use session::{RecoveryState, Session, SessionSpec, SessionStatus, StopReason};
pub use size::{InvalidSessionSize, SessionSize};
pub use store::{
    ActivityStore, ContextStore, CredentialSource, EnvironmentProfileStore, SessionRecord,
    SessionStore, SkillPack, SkillSource, StoreError, UserDirectoryStore,
};
pub use store::{PlatformSettingsStore, StoredSettings};
pub use terminal::{InvalidGeometry, ScreenModel, ScreenSnapshot, TerminalGeometry};
