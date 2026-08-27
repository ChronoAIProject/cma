//! Schema-only mirrors for Layer 2 values exposed by the REST interface.
//!
//! Layer 2 owns domain values and their Serde wire form, but it deliberately
//! has no dependency on the REST surface's OpenAPI generator. Keep these
//! shapes structurally aligned with those wire forms and point REST DTO fields
//! at them with `schema(value_type = ...)`.

#![allow(dead_code)]

use utoipa::ToSchema;

/// Lifecycle state of a session.
#[derive(serde::Serialize, ToSchema)]
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
#[derive(serde::Serialize, ToSchema)]
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
#[derive(serde::Serialize, ToSchema)]
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
#[derive(serde::Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum ReasoningEffort {
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
}

/// How much the agent may do without stopping to ask.
#[derive(serde::Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum PermissionMode {
    Guarded,
    Autonomous,
    Unrestricted,
}

/// Whose credential the agent spends on model calls.
#[derive(serde::Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum CredentialMode {
    Delegated,
    Own,
}

/// Which plane a session is driven on.
#[derive(serde::Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum SessionMode {
    Terminal,
    Conversation,
}

/// How much a session's runtime reserves.
#[derive(serde::Serialize, ToSchema)]
pub(super) struct SessionSize {
    /// Processor time, in thousandths of one core.
    #[schema(minimum = 1)]
    cpu_millis: u32,
    /// Working memory, in mebibytes.
    #[schema(minimum = 1)]
    memory_mib: u32,
}

/// Display material for the subject that created a session.
#[derive(serde::Serialize, ToSchema)]
pub(super) struct PublicIdentity {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    avatar_url: Option<String>,
}

/// Something the service can be asked to do.
#[derive(serde::Serialize, ToSchema)]
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
    TriggerPreviewRead,
    TriggerDraftCreate,
    TriggerDraftRead,
    TriggerDraftList,
    TriggerDraftUpdate,
    TriggerRevisionPublish,
    TriggerRevisionRead,
    TriggerRevisionList,
    TriggerRevisionRetire,
    TriggerConnectionList,
    TriggerConnectionBegin,
    TriggerConnectionReconnect,
    TriggerConnectionPreflight,
    TriggerRepositoryResolve,
    TriggerLaunchCreate,
    TriggerLaunchRead,
    TriggerLaunchList,
    TriggerLaunchDraftUpdate,
    TriggerLaunchConnectionSelect,
    TriggerLaunchStart,
    TriggerLaunchDiffReview,
    TriggerLaunchDiffRead,
    TriggerLaunchContributionPublish,
    TriggerLaunchPullRequestReview,
    TriggerLaunchPullRequestOpen,
    TriggerLaunchPullRequestUpdate,
    TriggerLaunchDiffDownload,
    TriggerLaunchCancel,
    TriggerLaunchPromptRecover,
    TriggerLaunchEventsRead,
    TriggerLaunchSupportRead,
    TriggerOrchestrationPause,
    TriggerOrchestrationResume,
    TriggerEffectExecute,
    TriggerEffectReconcile,
    TriggerEffectRecoveryAuthorize,
    TriggerCapabilityRenew,
    TriggerCapabilityRevoke,
    TriggerCleanupPlan,
    TriggerCleanupExecute,
}

/// Identifier of a session.
#[derive(serde::Serialize, ToSchema)]
pub(super) struct SessionId(String);

/// Identifier of a reusable environment profile.
#[derive(serde::Serialize, ToSchema)]
pub(super) struct EnvironmentProfileId(String);

/// Identifier asserted by the identity provider.
#[derive(serde::Serialize, ToSchema)]
pub(super) struct SubjectId(String);

#[derive(serde::Serialize, ToSchema)]
pub(super) struct TriggerId(String);

#[derive(serde::Serialize, ToSchema)]
pub(super) struct TriggerRevisionId(String);

#[derive(serde::Serialize, ToSchema)]
pub(super) struct PublicTriggerId(String);

#[derive(serde::Serialize, ToSchema)]
pub(super) struct LaunchId(String);

/// What an activity was taken on.
#[derive(serde::Serialize, ToSchema)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub(super) enum ActivityTarget {
    Session(SessionId),
    EnvironmentProfile(EnvironmentProfileId),
    Subject(SubjectId),
    Trigger(TriggerId),
    TriggerRevision(TriggerRevisionId),
    PublicTrigger(PublicTriggerId),
    Launch(LaunchId),
    TriggerConnection(String),
}

/// How an activity came out.
#[derive(serde::Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum ActivityOutcome {
    Succeeded,
    Denied,
    Rejected,
    Failed,
}

/// Every deployment-wide setting exposed through the administration API.
#[derive(serde::Serialize, ToSchema)]
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
#[derive(serde::Serialize, ToSchema)]
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
#[derive(serde::Serialize, ToSchema)]
pub(super) struct DeploymentCapacity {
    cpu_millis: Option<u64>,
    memory_mib: Option<u64>,
    assumed_session_size: Option<SessionSize>,
}

/// The deployment's sustained memory-pressure policy.
#[derive(serde::Serialize, ToSchema)]
pub(super) struct MemoryPressurePolicy {
    enter_permille: u16,
    exit_permille: u16,
    enter_samples: u16,
    exit_samples: u16,
    settle_seconds: u32,
}

/// Every rate the deployment has charged, oldest first.
#[derive(serde::Serialize, ToSchema)]
pub(super) struct PriceList {
    cards: Vec<RateCard>,
}

/// One sealed rate card.
#[derive(serde::Serialize, ToSchema)]
pub(super) struct RateCard {
    id: RateCardId,
    effective_from: String,
    rate: Rate,
    retired_at: Option<String>,
}

/// Identifier of one sealed rate card.
#[derive(serde::Serialize, ToSchema)]
pub(super) struct RateCardId(String);

/// A capacity rate in micro-USD.
#[derive(serde::Serialize, ToSchema)]
pub(super) struct Rate {
    cpu_micros_per_cpu_hour: u64,
    memory_micros_per_gib_hour: u64,
}

/// The per-session size range the deployment accepts.
#[derive(serde::Serialize, ToSchema)]
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
    use std::{collections::BTreeSet, num::NonZeroU32};

    use layer2_core as core;
    use serde::Serialize;
    use serde_json::Value;
    use utoipa::PartialSchema;

    use super::*;

    fn wire(value: &impl Serialize) -> Value {
        serde_json::to_value(value).expect("a contract value serializes")
    }

    fn schema<T: PartialSchema>() -> Value {
        serde_json::to_value(T::schema()).expect("a generated schema serializes")
    }

    fn assert_wire_mirror(actual: &impl Serialize, mirror: &impl Serialize) {
        assert_eq!(wire(actual), wire(mirror));
    }

    fn assert_object_mirror<T: PartialSchema>(actual: &impl Serialize, mirror: &impl Serialize) {
        let actual = wire(actual);
        assert_eq!(actual, wire(mirror));

        let actual_properties = actual
            .as_object()
            .expect("the Layer 2 wire value is an object")
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        let generated = schema::<T>();
        let schema_properties = generated["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("the mirror schema is an object: {generated}"))
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        assert_eq!(schema_properties, actual_properties);
    }

    fn assert_string_enum<T: PartialSchema>(actual: &[impl Serialize]) {
        let actual = actual.iter().map(wire).collect::<Vec<_>>();
        let generated = schema::<T>();
        assert_eq!(
            generated["enum"],
            Value::Array(actual),
            "the generated enum must equal Layer 2's Serde vocabulary: {generated}"
        );
    }

    macro_rules! enum_mirror_test {
        (
            $test:ident,
            $actual:ty,
            $mirror:ty,
            [$($actual_variant:path => $mirror_variant:path),+ $(,)?]
        ) => {
            #[test]
            fn $test() {
                fn exhaustive(value: $actual) {
                    match value {
                        $($actual_variant => {}),+
                    }
                }

                let pairs = [$(($actual_variant, $mirror_variant)),+];
                let actual = pairs.iter().map(|(actual, _)| actual).collect::<Vec<_>>();
                assert_string_enum::<$mirror>(&actual);
                for (actual, mirror) in pairs {
                    assert_wire_mirror(&actual, &mirror);
                    exhaustive(actual);
                }
            }
        };
    }

    enum_mirror_test!(
        session_status_mirror_tracks_layer_2,
        core::SessionStatus,
        SessionStatus,
        [
            core::SessionStatus::Provisioning => SessionStatus::Provisioning,
            core::SessionStatus::Ready => SessionStatus::Ready,
            core::SessionStatus::Running => SessionStatus::Running,
            core::SessionStatus::Rotating => SessionStatus::Rotating,
            core::SessionStatus::Idle => SessionStatus::Idle,
            core::SessionStatus::Terminated => SessionStatus::Terminated,
        ]
    );

    enum_mirror_test!(
        recovery_state_mirror_tracks_layer_2,
        core::RecoveryState,
        RecoveryState,
        [
            core::RecoveryState::Normal => RecoveryState::Normal,
            core::RecoveryState::Idle => RecoveryState::Idle,
            core::RecoveryState::Recovering => RecoveryState::Recovering,
            core::RecoveryState::Degraded => RecoveryState::Degraded,
            core::RecoveryState::Unknown => RecoveryState::Unknown,
            core::RecoveryState::Retired => RecoveryState::Retired,
            core::RecoveryState::Invalid => RecoveryState::Invalid,
        ]
    );

    enum_mirror_test!(
        stop_reason_mirror_tracks_layer_2,
        core::StopReason,
        StopReason,
        [
            core::StopReason::EndTurn => StopReason::EndTurn,
            core::StopReason::UserTerminated => StopReason::UserTerminated,
            core::StopReason::IdleTimeout => StopReason::IdleTimeout,
            core::StopReason::LifetimeCap => StopReason::LifetimeCap,
            core::StopReason::RequiresAction => StopReason::RequiresAction,
            core::StopReason::Revoked => StopReason::Revoked,
            core::StopReason::BudgetReached => StopReason::BudgetReached,
            core::StopReason::Error => StopReason::Error,
        ]
    );

    enum_mirror_test!(
        reasoning_effort_mirror_tracks_layer_2,
        core::ReasoningEffort,
        ReasoningEffort,
        [
            core::ReasoningEffort::Minimal => ReasoningEffort::Minimal,
            core::ReasoningEffort::Low => ReasoningEffort::Low,
            core::ReasoningEffort::Medium => ReasoningEffort::Medium,
            core::ReasoningEffort::High => ReasoningEffort::High,
            core::ReasoningEffort::XHigh => ReasoningEffort::XHigh,
        ]
    );

    enum_mirror_test!(
        permission_mode_mirror_tracks_layer_2,
        core::PermissionMode,
        PermissionMode,
        [
            core::PermissionMode::Guarded => PermissionMode::Guarded,
            core::PermissionMode::Autonomous => PermissionMode::Autonomous,
            core::PermissionMode::Unrestricted => PermissionMode::Unrestricted,
        ]
    );

    enum_mirror_test!(
        credential_mode_mirror_tracks_layer_2,
        core::CredentialMode,
        CredentialMode,
        [
            core::CredentialMode::Delegated => CredentialMode::Delegated,
            core::CredentialMode::Own => CredentialMode::Own,
        ]
    );

    enum_mirror_test!(
        session_mode_mirror_tracks_layer_2,
        core::SessionMode,
        SessionMode,
        [
            core::SessionMode::Terminal => SessionMode::Terminal,
            core::SessionMode::Conversation => SessionMode::Conversation,
        ]
    );

    enum_mirror_test!(
        activity_action_mirror_tracks_layer_2,
        core::ActivityAction,
        ActivityAction,
        [
            core::ActivityAction::SignIn => ActivityAction::SignIn,
            core::ActivityAction::SignOut => ActivityAction::SignOut,
            core::ActivityAction::SessionCreate => ActivityAction::SessionCreate,
            core::ActivityAction::SessionRead => ActivityAction::SessionRead,
            core::ActivityAction::SessionList => ActivityAction::SessionList,
            core::ActivityAction::SessionStop => ActivityAction::SessionStop,
            core::ActivityAction::SessionTerminate => ActivityAction::SessionTerminate,
            core::ActivityAction::SessionRevive => ActivityAction::SessionRevive,
            core::ActivityAction::SessionDelete => ActivityAction::SessionDelete,
            core::ActivityAction::SessionScreenRead => ActivityAction::SessionScreenRead,
            core::ActivityAction::SessionTranscriptRead => ActivityAction::SessionTranscriptRead,
            core::ActivityAction::SessionEventsRead => ActivityAction::SessionEventsRead,
            core::ActivityAction::SessionTerminalAttach => ActivityAction::SessionTerminalAttach,
            core::ActivityAction::SessionInput => ActivityAction::SessionInput,
            core::ActivityAction::SessionInterrupt => ActivityAction::SessionInterrupt,
            core::ActivityAction::SessionSteer => ActivityAction::SessionSteer,
            core::ActivityAction::SessionResize => ActivityAction::SessionResize,
            core::ActivityAction::SessionPlaneChange => ActivityAction::SessionPlaneChange,
            core::ActivityAction::SessionApprovalResolve => ActivityAction::SessionApprovalResolve,
            core::ActivityAction::SessionUsageRead => ActivityAction::SessionUsageRead,
            core::ActivityAction::EnvironmentProfileCreate => ActivityAction::EnvironmentProfileCreate,
            core::ActivityAction::EnvironmentProfileRead => ActivityAction::EnvironmentProfileRead,
            core::ActivityAction::EnvironmentProfileList => ActivityAction::EnvironmentProfileList,
            core::ActivityAction::EnvironmentProfileReplace => ActivityAction::EnvironmentProfileReplace,
            core::ActivityAction::EnvironmentProfileDelete => ActivityAction::EnvironmentProfileDelete,
            core::ActivityAction::SkillCatalogRead => ActivityAction::SkillCatalogRead,
            core::ActivityAction::RuntimeInventoryRead => ActivityAction::RuntimeInventoryRead,
            core::ActivityAction::UserDirectoryRead => ActivityAction::UserDirectoryRead,
            core::ActivityAction::ActivityLogRead => ActivityAction::ActivityLogRead,
            core::ActivityAction::PlatformSettingsRead => ActivityAction::PlatformSettingsRead,
            core::ActivityAction::PlatformSettingsWrite => ActivityAction::PlatformSettingsWrite,
            core::ActivityAction::PlatformSettingsReset => ActivityAction::PlatformSettingsReset,
            core::ActivityAction::TriggerPreviewRead => ActivityAction::TriggerPreviewRead,
            core::ActivityAction::TriggerDraftCreate => ActivityAction::TriggerDraftCreate,
            core::ActivityAction::TriggerDraftRead => ActivityAction::TriggerDraftRead,
            core::ActivityAction::TriggerDraftList => ActivityAction::TriggerDraftList,
            core::ActivityAction::TriggerDraftUpdate => ActivityAction::TriggerDraftUpdate,
            core::ActivityAction::TriggerRevisionPublish => ActivityAction::TriggerRevisionPublish,
            core::ActivityAction::TriggerRevisionRead => ActivityAction::TriggerRevisionRead,
            core::ActivityAction::TriggerRevisionList => ActivityAction::TriggerRevisionList,
            core::ActivityAction::TriggerRevisionRetire => ActivityAction::TriggerRevisionRetire,
            core::ActivityAction::TriggerConnectionList => ActivityAction::TriggerConnectionList,
            core::ActivityAction::TriggerConnectionBegin => ActivityAction::TriggerConnectionBegin,
            core::ActivityAction::TriggerConnectionReconnect => ActivityAction::TriggerConnectionReconnect,
            core::ActivityAction::TriggerConnectionPreflight => ActivityAction::TriggerConnectionPreflight,
            core::ActivityAction::TriggerRepositoryResolve => ActivityAction::TriggerRepositoryResolve,
            core::ActivityAction::TriggerLaunchCreate => ActivityAction::TriggerLaunchCreate,
            core::ActivityAction::TriggerLaunchRead => ActivityAction::TriggerLaunchRead,
            core::ActivityAction::TriggerLaunchList => ActivityAction::TriggerLaunchList,
            core::ActivityAction::TriggerLaunchDraftUpdate => ActivityAction::TriggerLaunchDraftUpdate,
            core::ActivityAction::TriggerLaunchConnectionSelect => ActivityAction::TriggerLaunchConnectionSelect,
            core::ActivityAction::TriggerLaunchStart => ActivityAction::TriggerLaunchStart,
            core::ActivityAction::TriggerLaunchDiffReview => ActivityAction::TriggerLaunchDiffReview,
            core::ActivityAction::TriggerLaunchDiffRead => ActivityAction::TriggerLaunchDiffRead,
            core::ActivityAction::TriggerLaunchContributionPublish => ActivityAction::TriggerLaunchContributionPublish,
            core::ActivityAction::TriggerLaunchPullRequestReview => ActivityAction::TriggerLaunchPullRequestReview,
            core::ActivityAction::TriggerLaunchPullRequestOpen => ActivityAction::TriggerLaunchPullRequestOpen,
            core::ActivityAction::TriggerLaunchPullRequestUpdate => ActivityAction::TriggerLaunchPullRequestUpdate,
            core::ActivityAction::TriggerLaunchDiffDownload => ActivityAction::TriggerLaunchDiffDownload,
            core::ActivityAction::TriggerLaunchCancel => ActivityAction::TriggerLaunchCancel,
            core::ActivityAction::TriggerLaunchPromptRecover => ActivityAction::TriggerLaunchPromptRecover,
            core::ActivityAction::TriggerLaunchEventsRead => ActivityAction::TriggerLaunchEventsRead,
            core::ActivityAction::TriggerLaunchSupportRead => ActivityAction::TriggerLaunchSupportRead,
            core::ActivityAction::TriggerOrchestrationPause => ActivityAction::TriggerOrchestrationPause,
            core::ActivityAction::TriggerOrchestrationResume => ActivityAction::TriggerOrchestrationResume,
            core::ActivityAction::TriggerEffectExecute => ActivityAction::TriggerEffectExecute,
            core::ActivityAction::TriggerEffectReconcile => ActivityAction::TriggerEffectReconcile,
            core::ActivityAction::TriggerEffectRecoveryAuthorize => ActivityAction::TriggerEffectRecoveryAuthorize,
            core::ActivityAction::TriggerCapabilityRenew => ActivityAction::TriggerCapabilityRenew,
            core::ActivityAction::TriggerCapabilityRevoke => ActivityAction::TriggerCapabilityRevoke,
            core::ActivityAction::TriggerCleanupPlan => ActivityAction::TriggerCleanupPlan,
            core::ActivityAction::TriggerCleanupExecute => ActivityAction::TriggerCleanupExecute,
        ]
    );

    enum_mirror_test!(
        activity_outcome_mirror_tracks_layer_2,
        core::ActivityOutcome,
        ActivityOutcome,
        [
            core::ActivityOutcome::Succeeded => ActivityOutcome::Succeeded,
            core::ActivityOutcome::Denied => ActivityOutcome::Denied,
            core::ActivityOutcome::Rejected => ActivityOutcome::Rejected,
            core::ActivityOutcome::Failed => ActivityOutcome::Failed,
        ]
    );

    #[test]
    fn activity_target_and_identifier_mirrors_track_layer_2() {
        fn exhaustive(value: core::ActivityTarget) {
            match value {
                core::ActivityTarget::Session(_) => {}
                core::ActivityTarget::EnvironmentProfile(_) => {}
                core::ActivityTarget::Subject(_) => {}
                core::ActivityTarget::Trigger(_) => {}
                core::ActivityTarget::TriggerRevision(_) => {}
                core::ActivityTarget::PublicTrigger(_) => {}
                core::ActivityTarget::Launch(_) => {}
                core::ActivityTarget::TriggerConnection(_) => {}
            }
        }

        let pairs = [
            (
                core::ActivityTarget::Session(core::SessionId("sesn_schema".into())),
                ActivityTarget::Session(SessionId("sesn_schema".into())),
            ),
            (
                core::ActivityTarget::EnvironmentProfile(core::EnvironmentProfileId(
                    "envp_schema".into(),
                )),
                ActivityTarget::EnvironmentProfile(EnvironmentProfileId("envp_schema".into())),
            ),
            (
                core::ActivityTarget::Subject(core::SubjectId("subject-schema".into())),
                ActivityTarget::Subject(SubjectId("subject-schema".into())),
            ),
            (
                core::ActivityTarget::Trigger(core::TriggerId("trigger-schema".into())),
                ActivityTarget::Trigger(TriggerId("trigger-schema".into())),
            ),
            (
                core::ActivityTarget::TriggerRevision(core::TriggerRevisionId(
                    "revision-schema".into(),
                )),
                ActivityTarget::TriggerRevision(TriggerRevisionId("revision-schema".into())),
            ),
            (
                core::ActivityTarget::PublicTrigger(core::PublicTriggerId("public-schema".into())),
                ActivityTarget::PublicTrigger(PublicTriggerId("public-schema".into())),
            ),
            (
                core::ActivityTarget::Launch(core::LaunchId("launch-schema".into())),
                ActivityTarget::Launch(LaunchId("launch-schema".into())),
            ),
            (
                core::ActivityTarget::TriggerConnection("service-schema".into()),
                ActivityTarget::TriggerConnection("service-schema".into()),
            ),
        ];
        let expected_kinds = pairs
            .iter()
            .map(|(actual, _)| {
                wire(actual)["kind"]
                    .as_str()
                    .expect("an activity target kind is text")
                    .to_string()
            })
            .collect::<BTreeSet<_>>();

        fn collect_enums(value: &Value, found: &mut BTreeSet<String>) {
            match value {
                Value::Object(object) => {
                    if let Some(values) = object.get("enum").and_then(Value::as_array) {
                        found.extend(values.iter().filter_map(Value::as_str).map(str::to_string));
                    }
                    for value in object.values() {
                        collect_enums(value, found);
                    }
                }
                Value::Array(values) => {
                    for value in values {
                        collect_enums(value, found);
                    }
                }
                _ => {}
            }
        }

        let mut schema_kinds = BTreeSet::new();
        collect_enums(&schema::<ActivityTarget>(), &mut schema_kinds);
        assert_eq!(schema_kinds, expected_kinds);
        for (actual, mirror) in pairs {
            assert_wire_mirror(&actual, &mirror);
            exhaustive(actual);
        }

        assert_wire_mirror(
            &core::SessionId("sesn_schema".into()),
            &SessionId("sesn_schema".into()),
        );
        assert_wire_mirror(
            &core::EnvironmentProfileId("envp_schema".into()),
            &EnvironmentProfileId("envp_schema".into()),
        );
        assert_wire_mirror(
            &core::SubjectId("subject-schema".into()),
            &SubjectId("subject-schema".into()),
        );
        assert_wire_mirror(
            &core::TriggerId("trigger-schema".into()),
            &TriggerId("trigger-schema".into()),
        );
        assert_wire_mirror(
            &core::TriggerRevisionId("revision-schema".into()),
            &TriggerRevisionId("revision-schema".into()),
        );
        assert_wire_mirror(
            &core::PublicTriggerId("public-schema".into()),
            &PublicTriggerId("public-schema".into()),
        );
        assert_wire_mirror(
            &core::LaunchId("launch-schema".into()),
            &LaunchId("launch-schema".into()),
        );
        for generated in [
            schema::<SessionId>(),
            schema::<EnvironmentProfileId>(),
            schema::<SubjectId>(),
            schema::<TriggerId>(),
            schema::<TriggerRevisionId>(),
            schema::<PublicTriggerId>(),
            schema::<LaunchId>(),
        ] {
            assert_eq!(generated["type"], "string");
        }
    }

    #[test]
    fn public_identity_mirror_tracks_layer_2() {
        let actual = core::PublicIdentity {
            display_name: Some("Schema Reader".into()),
            avatar_url: Some("https://example.test/avatar.png".into()),
        };
        let mirror = PublicIdentity {
            display_name: Some("Schema Reader".into()),
            avatar_url: Some("https://example.test/avatar.png".into()),
        };
        assert_object_mirror::<PublicIdentity>(&actual, &mirror);
        assert_wire_mirror(
            &core::PublicIdentity {
                display_name: None,
                avatar_url: None,
            },
            &PublicIdentity {
                display_name: None,
                avatar_url: None,
            },
        );
    }

    fn actual_settings() -> core::PlatformSettings {
        core::PlatformSettings {
            briefing: core::SessionBriefing {
                before: vec!["before".into()],
                reach_heading: "reach heading".into(),
                reach_lead: "reach lead".into(),
                reach_close: "reach close".into(),
                reach_none: "reach none".into(),
                after: vec!["after".into()],
            },
            deployment_capacity: core::DeploymentCapacity {
                cpu_millis: Some(8_000),
                memory_mib: Some(16_384),
                assumed_session_size: Some(
                    core::SessionSize::new(1_000, 2_048).expect("test size is non-zero"),
                ),
            },
            memory_pressure: core::MemoryPressurePolicy::new(800, 700, 5, 15, 120)
                .expect("test pressure policy is valid"),
            pricing: core::PriceList {
                cards: vec![core::RateCard {
                    id: core::RateCardId("rate_schema".into()),
                    effective_from: "2026-08-25T00:00:00Z".into(),
                    rate: core::Rate {
                        cpu_micros_per_cpu_hour: 7,
                        memory_micros_per_gib_hour: 11,
                    },
                    retired_at: Some("2026-08-26T00:00:00Z".into()),
                }],
            },
            size_bounds: core::CapacityBounds {
                min_cpu_millis: NonZeroU32::new(250).expect("non-zero"),
                max_cpu_millis: NonZeroU32::new(4_000).expect("non-zero"),
                min_memory_mib: NonZeroU32::new(256).expect("non-zero"),
                max_memory_mib: NonZeroU32::new(8_192).expect("non-zero"),
            },
            live_sessions: NonZeroU32::new(9),
        }
    }

    fn mirror_settings() -> PlatformSettings {
        PlatformSettings {
            briefing: SessionBriefing {
                before: vec!["before".into()],
                reach_heading: "reach heading".into(),
                reach_lead: "reach lead".into(),
                reach_close: "reach close".into(),
                reach_none: "reach none".into(),
                after: vec!["after".into()],
            },
            deployment_capacity: DeploymentCapacity {
                cpu_millis: Some(8_000),
                memory_mib: Some(16_384),
                assumed_session_size: Some(SessionSize {
                    cpu_millis: 1_000,
                    memory_mib: 2_048,
                }),
            },
            memory_pressure: MemoryPressurePolicy {
                enter_permille: 800,
                exit_permille: 700,
                enter_samples: 5,
                exit_samples: 15,
                settle_seconds: 120,
            },
            pricing: PriceList {
                cards: vec![RateCard {
                    id: RateCardId("rate_schema".into()),
                    effective_from: "2026-08-25T00:00:00Z".into(),
                    rate: Rate {
                        cpu_micros_per_cpu_hour: 7,
                        memory_micros_per_gib_hour: 11,
                    },
                    retired_at: Some("2026-08-26T00:00:00Z".into()),
                }],
            },
            size_bounds: CapacityBounds {
                min_cpu_millis: 250,
                max_cpu_millis: 4_000,
                min_memory_mib: 256,
                max_memory_mib: 8_192,
            },
            live_sessions: Some(9),
        }
    }

    #[test]
    fn platform_setting_mirrors_track_layer_2() {
        let actual = actual_settings();
        let mirror = mirror_settings();
        assert_object_mirror::<PlatformSettings>(&actual, &mirror);
        assert_object_mirror::<SessionBriefing>(&actual.briefing, &mirror.briefing);
        assert_object_mirror::<DeploymentCapacity>(
            &actual.deployment_capacity,
            &mirror.deployment_capacity,
        );
        assert_object_mirror::<SessionSize>(
            actual
                .deployment_capacity
                .assumed_session_size
                .as_ref()
                .expect("the test capacity has an assumed size"),
            mirror
                .deployment_capacity
                .assumed_session_size
                .as_ref()
                .expect("the mirror capacity has an assumed size"),
        );
        assert_object_mirror::<MemoryPressurePolicy>(
            &actual.memory_pressure,
            &mirror.memory_pressure,
        );
        assert_object_mirror::<PriceList>(&actual.pricing, &mirror.pricing);
        let actual_card = &actual.pricing.cards[0];
        let mirror_card = &mirror.pricing.cards[0];
        assert_object_mirror::<RateCard>(actual_card, mirror_card);
        assert_wire_mirror(&actual_card.id, &mirror_card.id);
        assert_eq!(schema::<RateCardId>()["type"], "string");
        assert_object_mirror::<Rate>(&actual_card.rate, &mirror_card.rate);
        assert_object_mirror::<CapacityBounds>(&actual.size_bounds, &mirror.size_bounds);
    }

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
