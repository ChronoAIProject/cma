//! Schema-only mirrors for Layer 2 values exposed by the REST interface.
//!
//! Layer 2 owns domain values and their Serde wire form, but it deliberately
//! has no dependency on the REST surface's OpenAPI generator. Keep these
//! shapes structurally aligned with those wire forms and point REST DTO fields
//! at them with `schema(value_type = ...)`.

#![allow(dead_code)]

use utoipa::ToSchema;

/// Lifecycle state of a session.
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum SessionStatus {
    Provisioning,
    Ready,
    Running,
    Rotating,
    Idle,
    Terminated,
}

/// Operational health of a session.
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum RecoveryState {
    Normal,
    Idle,
    Recovering,
    Degraded,
    Unknown,
    Retired,
    Invalid,
}

/// Why a session stopped or paused.
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum StopReason {
    EndTurn,
    UserTerminated,
    IdleTimeout,
    LifetimeCap,
    RequiresAction,
    Revoked,
    BudgetReached,
    Error,
}

/// How much deliberation to ask of the model.
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum ReasoningEffort {
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
}

/// How much the agent may do without stopping to ask.
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum PermissionMode {
    Guarded,
    Autonomous,
    Unrestricted,
}

/// Whose credential the agent spends on model calls.
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum CredentialMode {
    Delegated,
    Own,
}

/// Which plane a session is driven on.
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum SessionMode {
    Terminal,
    Conversation,
}

/// How much a session's runtime reserves.
#[derive(ToSchema)]
pub(super) struct SessionSize {
    /// Processor time, in thousandths of one core.
    #[schema(minimum = 1)]
    cpu_millis: u32,
    /// Working memory, in mebibytes.
    #[schema(minimum = 1)]
    memory_mib: u32,
}

/// Display material for the subject that created a session.
#[derive(ToSchema)]
pub(super) struct PublicIdentity {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    avatar_url: Option<String>,
}

/// Something the service can be asked to do.
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum ActivityAction {
    SignIn,
    SignOut,
    SessionCreate,
    SessionRead,
    SessionList,
    SessionStop,
    SessionTerminate,
    SessionRevive,
    SessionDelete,
    SessionScreenRead,
    SessionTranscriptRead,
    SessionEventsRead,
    SessionTerminalAttach,
    SessionInput,
    SessionInterrupt,
    SessionSteer,
    SessionResize,
    SessionPlaneChange,
    SessionApprovalResolve,
    SessionUsageRead,
    EnvironmentProfileCreate,
    EnvironmentProfileRead,
    EnvironmentProfileList,
    EnvironmentProfileReplace,
    EnvironmentProfileDelete,
    SkillCatalogRead,
    RuntimeInventoryRead,
    UserDirectoryRead,
    ActivityLogRead,
    PlatformSettingsRead,
    PlatformSettingsWrite,
    PlatformSettingsReset,
}

/// Identifier of a session.
#[derive(ToSchema)]
pub(super) struct SessionId(String);

/// Identifier of a reusable environment profile.
#[derive(ToSchema)]
pub(super) struct EnvironmentProfileId(String);

/// Identifier asserted by the identity provider.
#[derive(ToSchema)]
pub(super) struct SubjectId(String);

/// What an activity was taken on.
#[derive(ToSchema)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub(super) enum ActivityTarget {
    Session(SessionId),
    EnvironmentProfile(EnvironmentProfileId),
    Subject(SubjectId),
}

/// How an activity came out.
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum ActivityOutcome {
    Succeeded,
    Denied,
    Rejected,
    Failed,
}

/// Every deployment-wide setting exposed through the administration API.
#[derive(ToSchema)]
pub(super) struct PlatformSettings {
    #[serde(default)]
    briefing: SessionBriefing,
    #[serde(default)]
    deployment_capacity: DeploymentCapacity,
    #[serde(default)]
    memory_pressure: MemoryPressurePolicy,
    #[serde(default)]
    pricing: PriceList,
    #[serde(default)]
    size_bounds: CapacityBounds,
    #[serde(default)]
    #[schema(value_type = Option<u32>, minimum = 1)]
    live_sessions: Option<u32>,
}

/// Editable prose around the generated granted-services section.
#[derive(ToSchema)]
pub(super) struct SessionBriefing {
    #[serde(default)]
    before: Vec<String>,
    #[serde(default)]
    reach_heading: String,
    #[serde(default)]
    reach_lead: String,
    #[serde(default)]
    reach_close: String,
    #[serde(default)]
    reach_none: String,
    #[serde(default)]
    after: Vec<String>,
}

/// The declared reservation ceiling for the deployment.
#[derive(ToSchema)]
pub(super) struct DeploymentCapacity {
    cpu_millis: Option<u64>,
    memory_mib: Option<u64>,
    assumed_session_size: Option<SessionSize>,
}

/// The deployment's sustained memory-pressure policy.
#[derive(ToSchema)]
pub(super) struct MemoryPressurePolicy {
    enter_permille: u16,
    exit_permille: u16,
    enter_samples: u16,
    exit_samples: u16,
    settle_seconds: u32,
}

/// Every rate the deployment has charged, oldest first.
#[derive(ToSchema)]
pub(super) struct PriceList {
    cards: Vec<RateCard>,
}

/// One sealed rate card.
#[derive(ToSchema)]
pub(super) struct RateCard {
    id: RateCardId,
    effective_from: String,
    rate: Rate,
    retired_at: Option<String>,
}

/// Identifier of one sealed rate card.
#[derive(ToSchema)]
pub(super) struct RateCardId(String);

/// A capacity rate in micro-USD.
#[derive(ToSchema)]
pub(super) struct Rate {
    cpu_micros_per_cpu_hour: u64,
    memory_micros_per_gib_hour: u64,
}

/// The per-session size range the deployment accepts.
#[derive(ToSchema)]
pub(super) struct CapacityBounds {
    #[schema(minimum = 1)]
    min_cpu_millis: u32,
    #[schema(minimum = 1)]
    max_cpu_millis: u32,
    #[schema(minimum = 1)]
    min_memory_mib: u32,
    #[schema(minimum = 1)]
    max_memory_mib: u32,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn schema_mirror_component_names_do_not_collide() {
        let names = [
            SessionStatus::name(),
            RecoveryState::name(),
            StopReason::name(),
            ReasoningEffort::name(),
            PermissionMode::name(),
            CredentialMode::name(),
            SessionMode::name(),
            SessionSize::name(),
            PublicIdentity::name(),
            ActivityAction::name(),
            SessionId::name(),
            EnvironmentProfileId::name(),
            SubjectId::name(),
            ActivityTarget::name(),
            ActivityOutcome::name(),
            PlatformSettings::name(),
            SessionBriefing::name(),
            DeploymentCapacity::name(),
            MemoryPressurePolicy::name(),
            PriceList::name(),
            RateCard::name(),
            RateCardId::name(),
            Rate::name(),
            CapacityBounds::name(),
        ];
        let unique: BTreeSet<_> = names.iter().map(|name| name.as_ref()).collect();
        assert_eq!(unique.len(), names.len());
    }
}
