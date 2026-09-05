use crate::sandbox::FirstMessageDelivery;
use crate::{AgentId, LaunchId, SandboxId, ServiceHandoffId, ServiceId, SubjectId, WorkspaceId};

use super::*;

const NOW: &str = "2026-09-05T00:00:00Z";
const LATER: &str = "2026-09-05T00:01:00Z";
const LATEST: &str = "2026-09-05T00:02:00Z";

fn digest(byte: char) -> String {
    byte.to_string().repeat(64)
}

fn requirements(states: &[(&str, ServiceRequirementState)]) -> RequirementsSnapshot {
    RequirementsSnapshot {
        services: states
            .iter()
            .map(|(service, state)| ServiceRequirement {
                service: ServiceId::new(*service),
                state: *state,
            })
            .collect(),
        checked_at: NOW.into(),
    }
}

fn seed(review: ReviewRequirements) -> LaunchSeed {
    LaunchSeed {
        trigger_id: TriggerId("trg_one".into()),
        revision_digest: digest('a'),
        public_id: PublicTriggerId("pub_one".into()),
        owner: SubjectId::new("owner"),
        placement_id: Some(PlacementId("plc_one".into())),
        referrer_origin: Some("https://example.com".into()),
        requirements: requirements(&[]),
        review,
        resolution: LaunchResolution::pending(),
        has_first_message: true,
        command_key_hash: digest('b'),
        command_fingerprint: digest('c'),
        created_at: NOW.into(),
    }
}

fn confirmation(surface: LaunchConsentSurface) -> LaunchConfirmation {
    LaunchConfirmation {
        surface,
        trust_confirmed_digest: None,
        acknowledgements: Vec::new(),
        confirmed_at: LATER.into(),
    }
}

#[test]
fn review_needs_are_closed_ordered_and_deduplicated() {
    let snapshot = requirements(&[
        ("service-a", ServiceRequirementState::Missing),
        ("service-b", ServiceRequirementState::Inactive),
        ("service-c", ServiceRequirementState::Satisfied),
    ]);
    let review = ReviewRequirements::build(&snapshot, Some(digest('d')), true, true).unwrap();
    assert!(!review.can_start_now);
    assert_eq!(
        review.needs,
        [
            ReviewNeed::Connection {
                service: ServiceId::new("service-a")
            },
            ReviewNeed::Connection {
                service: ServiceId::new("service-b")
            },
            ReviewNeed::TrustConfirmation {
                commands_digest: digest('d')
            },
            ReviewNeed::OwnerRequiresReview,
            ReviewNeed::RepeatChoice,
        ]
    );

    let duplicate = requirements(&[
        ("service-a", ServiceRequirementState::Missing),
        ("service-a", ServiceRequirementState::Satisfied),
    ]);
    assert_eq!(
        duplicate.validate().unwrap_err().code,
        "invalid_launch_requirements"
    );
}

#[test]
fn launch_starts_with_the_exact_progress_vocabulary() {
    let review = ReviewRequirements::build(&requirements(&[]), None, false, false).unwrap();
    let launch = Launch::new(seed(review)).unwrap();
    assert_eq!(launch.state, LaunchState::Draft);
    assert_eq!(launch.first_message, FirstMessageDelivery::Queued);
    assert_eq!(
        launch.steps.iter().map(|step| step.key).collect::<Vec<_>>(),
        [
            LaunchStepKey::SigningIn,
            LaunchStepKey::CheckingRequirements,
            LaunchStepKey::Sandbox,
            LaunchStepKey::Workspace,
            LaunchStepKey::Agent,
            LaunchStepKey::FirstMessage,
            LaunchStepKey::Ready,
        ]
    );
    assert_eq!(launch.steps[0].status, LaunchStepStatus::Completed);
    assert_eq!(launch.steps[2].status, LaunchStepStatus::Pending);
}

#[test]
fn start_requires_a_clear_review_and_records_consent() {
    let blocked = ReviewRequirements::build(&requirements(&[]), None, true, false).unwrap();
    let mut launch = Launch::new(seed(blocked)).unwrap();
    assert_eq!(
        launch
            .start(confirmation(LaunchConsentSurface::Landing))
            .unwrap_err()
            .code,
        "launch_not_startable"
    );

    launch.review = ReviewRequirements::build(&requirements(&[]), None, false, false).unwrap();
    launch
        .start(confirmation(LaunchConsentSurface::Landing))
        .unwrap();
    assert_eq!(launch.state, LaunchState::Resolving);
    assert_eq!(launch.work.step, LaunchWorkStep::ResolveSandbox);
    assert_eq!(launch.started_at.as_deref(), Some(LATER));
    assert_eq!(launch.confirmations.len(), 1);
    launch.validate().unwrap();
}

#[test]
fn transition_graph_refuses_skips_and_keeps_ready_history() {
    let review = ReviewRequirements::build(&requirements(&[]), None, false, false).unwrap();
    let mut launch = Launch::new(seed(review)).unwrap();
    launch
        .start(confirmation(LaunchConsentSurface::Review))
        .unwrap();
    assert_eq!(
        launch
            .transition(LaunchState::AgentPending, LATEST.into())
            .unwrap_err()
            .code,
        "invalid_launch_transition"
    );
    launch
        .transition(LaunchState::SandboxPending, LATEST.into())
        .unwrap();
    launch
        .transition(LaunchState::WorkspacePending, LATEST.into())
        .unwrap();
    launch
        .transition(LaunchState::AgentPending, LATEST.into())
        .unwrap();
    launch
        .transition(LaunchState::Ready, LATEST.into())
        .unwrap();
    launch
        .transition(LaunchState::Completed, LATEST.into())
        .unwrap();
    assert_eq!(launch.ready_at.as_deref(), Some(LATEST));
    assert_eq!(launch.work.step, LaunchWorkStep::None);
    launch.validate().unwrap();
}

#[test]
fn cancellation_is_allowed_before_start_but_never_after_ready() {
    let review = ReviewRequirements::build(&requirements(&[]), None, false, false).unwrap();
    let mut draft = Launch::new(seed(review.clone())).unwrap();
    draft.cancel(LATER.into()).unwrap();
    assert_eq!(draft.state, LaunchState::Cancelled);
    assert!(draft.started_at.is_none());
    draft.validate().unwrap();

    let mut ready = Launch::new(seed(review)).unwrap();
    ready
        .start(confirmation(LaunchConsentSurface::Landing))
        .unwrap();
    ready.transition(LaunchState::Ready, LATEST.into()).unwrap();
    assert_eq!(
        ready.cancel(LATEST.into()).unwrap_err().code,
        "launch_not_cancellable"
    );
}

#[test]
fn resolution_cannot_escape_its_parent_lineage() {
    let invalid = LaunchResolution {
        sandbox: ResolvedResource::Pending,
        workspace: ResolvedResource::Reused(WorkspaceId("wks_one".into())),
        agent: ResolvedResource::Pending,
        choice_required: false,
    };
    assert_eq!(invalid.validate().unwrap_err().field, "resolution.sandbox");

    let invalid = LaunchResolution {
        sandbox: ResolvedResource::Reused(SandboxId("sbx_one".into())),
        workspace: ResolvedResource::Reused(WorkspaceId("wks_one".into())),
        agent: ResolvedResource::Reused(AgentId("agt_one".into())),
        choice_required: true,
    };
    assert_eq!(
        invalid.validate().unwrap_err().field,
        "resolution.choice_required"
    );
}

#[test]
fn work_leases_fence_stale_effect_publication() {
    let mut work = LaunchWorkState::new(LaunchWorkStep::ResolveSandbox, NOW.into());
    let first = work.claim("worker-a".into(), NOW, LATER.into()).unwrap();
    work.prepare_effect(
        "worker-a",
        first,
        LaunchEffectKind::CreateSandbox,
        "launch:lch_one:sandbox".into(),
        NOW.into(),
    )
    .unwrap();
    work.mark_effect_executing("worker-a", first, NOW.into())
        .unwrap();

    let second = work.claim("worker-b".into(), LATER, LATEST.into()).unwrap();
    assert!(second > first);
    assert_eq!(
        work.mark_effect_applied("worker-a", first, "sbx_one".into(), LATEST.into())
            .unwrap_err()
            .code,
        "stale_launch_fence"
    );
}

#[test]
fn handoffs_validate_the_provider_correlation_needed_by_storage() {
    let handoff = ServiceConnectionHandoff {
        id: ServiceHandoffId("hnd_one".into()),
        launch_id: LaunchId("lch_one".into()),
        owner: SubjectId::new("owner"),
        service_id: ServiceId::new("service-a"),
        intent_id: "intent-one".into(),
        hosted_url: "https://nyx.example/connect/one".into(),
        expires_at: LATER.into(),
        state: BrokerHandoffState::Pending,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        external_id: "provider-one".into(),
        revision: 1,
    };
    handoff.validate().unwrap();
    assert_eq!(
        serde_json::to_value(&handoff).unwrap()["external_id"],
        "provider-one"
    );
}
