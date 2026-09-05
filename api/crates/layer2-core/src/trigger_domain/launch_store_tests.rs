use crate::{
    AgentProfileId, LaunchId, SandboxProfileId, ServiceHandoffId, ServiceId, SubjectId,
    WorkspaceProfileId,
};

use super::*;
use crate::profiles::{AgentProfileRef, SandboxProfileRef, WorkspaceProfileRef};

const NOW: &str = "2026-09-05T00:00:00Z";
const LATER: &str = "2026-09-05T00:01:00Z";
const LATEST: &str = "2026-09-05T00:02:00Z";

fn digest(byte: char) -> String {
    byte.to_string().repeat(64)
}

fn content() -> TriggerContent {
    TriggerContent {
        presentation: Presentation {
            title: "Review this change".into(),
            description: String::new(),
            button_label: ButtonLabel::OpenInCma,
            theme: Theme::Auto,
            signup_hint: None,
        },
        target: TriggerTarget {
            sandbox_profile: SandboxProfileRef::new(SandboxProfileId("sbp_one".into()), 1),
            workspace_profile: WorkspaceProfileRef::new(WorkspaceProfileId("wkp_one".into()), 1),
            agent_profile: AgentProfileRef::new(AgentProfileId("agp_one".into()), 1),
        },
        reuse: ReusePolicy {
            sandbox: SandboxReusePolicy::ReuseExisting,
            workspace: WorkspaceReusePolicy::ReuseExisting,
            agent: AgentReusePolicy::AlwaysNew,
        },
        first_message: "Please review it.".into(),
        consent: TriggerConsent {
            always_review: false,
        },
    }
}

fn public_trigger() -> PublicTrigger {
    PublicTrigger {
        trigger_id: TriggerId("trg_one".into()),
        public_id: PublicTriggerId("pub_one".into()),
        owner: SubjectId::new("publisher"),
        availability: TriggerAvailability::Published,
        revision: TriggerRevision::new(1, content(), NOW.into()).unwrap(),
    }
}

fn launch(trigger: &PublicTrigger, key: char, fingerprint: char) -> Launch {
    Launch::new(LaunchSeed {
        trigger_id: trigger.trigger_id.clone(),
        revision_digest: trigger.revision.digest.clone(),
        public_id: trigger.public_id.clone(),
        owner: SubjectId::new("visitor"),
        placement_id: None,
        referrer_origin: None,
        requirements: RequirementsSnapshot {
            services: vec![],
            checked_at: NOW.into(),
        },
        review: ReviewRequirements {
            needs: vec![],
            can_start_now: true,
        },
        resolution: LaunchResolution::pending(),
        has_first_message: true,
        command_key_hash: digest(key),
        command_fingerprint: digest(fingerprint),
        created_at: NOW.into(),
    })
    .unwrap()
}

async fn admitted(store: &InMemoryLaunchStore) -> (PublicTrigger, Launch) {
    let trigger = public_trigger();
    let result = store
        .admit_launch(LaunchAdmission {
            launch: launch(&trigger, 'a', 'b'),
            trigger: trigger.clone(),
            events: vec![LaunchEventKind::Created],
        })
        .await
        .unwrap();
    (trigger, result.launch().clone())
}

#[tokio::test]
async fn admission_replays_exactly_and_counts_only_the_created_launch() {
    let store = InMemoryLaunchStore::default();
    let trigger = public_trigger();
    let first = launch(&trigger, 'a', 'b');
    let created = store
        .admit_launch(LaunchAdmission {
            launch: first.clone(),
            trigger: trigger.clone(),
            events: vec![LaunchEventKind::Created],
        })
        .await
        .unwrap();
    assert!(created.was_created());
    assert_eq!(created.launch().event_head, 1);

    let replay = store
        .admit_launch(LaunchAdmission {
            launch: launch(&trigger, 'a', 'b'),
            trigger: trigger.clone(),
            events: vec![LaunchEventKind::Created],
        })
        .await
        .unwrap();
    assert!(!replay.was_created());
    assert_eq!(replay.launch().id, first.id);
    assert_eq!(store.trigger_launch_count(&trigger.trigger_id), 1);

    assert_eq!(
        store
            .admit_launch(LaunchAdmission {
                launch: launch(&trigger, 'a', 'c'),
                trigger,
                events: vec![LaunchEventKind::Created],
            })
            .await,
        Err(LaunchStoreError::CommandConflict)
    );
}

#[tokio::test]
async fn commits_are_revision_fenced_and_append_contiguous_events() {
    let store = InMemoryLaunchStore::default();
    let (_, mut launch) = admitted(&store).await;
    let expected = launch.revision;
    launch
        .start(LaunchConfirmation {
            surface: LaunchConsentSurface::Landing,
            trust_confirmed_digest: None,
            acknowledgements: vec![],
            confirmed_at: LATER.into(),
        })
        .unwrap();
    let committed = store
        .commit_launch(LaunchCommit {
            expected_revision: expected,
            launch,
            event_at: LATER.into(),
            events: vec![
                LaunchEventKind::ConfirmationRecorded,
                LaunchEventKind::StateChanged {
                    from: LaunchState::Draft,
                    to: LaunchState::Resolving,
                },
            ],
        })
        .await
        .unwrap();
    assert_eq!(committed.event_head, 3);
    let events = store
        .launch_events(&committed.id, &committed.owner, 0, 100)
        .await
        .unwrap();
    assert_eq!(
        events.iter().map(|event| event.seq).collect::<Vec<_>>(),
        [1, 2, 3]
    );
    assert_eq!(
        store
            .commit_launch(LaunchCommit {
                expected_revision: expected,
                launch: committed.clone(),
                event_at: LATER.into(),
                events: vec![],
            })
            .await,
        Err(LaunchStoreError::Conflict {
            expected,
            actual: committed.revision,
        })
    );
    assert_eq!(
        store
            .launch_events(&committed.id, &committed.owner, 4, 100)
            .await,
        Err(LaunchStoreError::InvalidCursor)
    );
}

#[tokio::test]
async fn due_work_claims_are_atomic_and_expired_leases_can_be_taken_over() {
    let store = InMemoryLaunchStore::default();
    let (_, mut launch) = admitted(&store).await;
    let expected = launch.revision;
    launch
        .start(LaunchConfirmation {
            surface: LaunchConsentSurface::Landing,
            trust_confirmed_digest: None,
            acknowledgements: vec![],
            confirmed_at: NOW.into(),
        })
        .unwrap();
    let launch = store
        .commit_launch(LaunchCommit {
            expected_revision: expected,
            launch,
            event_at: NOW.into(),
            events: vec![LaunchEventKind::ConfirmationRecorded],
        })
        .await
        .unwrap();
    let due = store.due_launches(NOW, 10).await.unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0], launch.id);
    let first = store
        .claim_launch(&launch.id, "worker-a", NOW.into(), LATER.into())
        .await
        .unwrap()
        .unwrap();
    assert!(store
        .claim_launch(&launch.id, "worker-b", NOW.into(), LATER.into())
        .await
        .unwrap()
        .is_none());
    let second = store
        .claim_launch(&launch.id, "worker-b", LATER.into(), LATEST.into())
        .await
        .unwrap()
        .unwrap();
    assert!(second.work.lease.as_ref().unwrap().fence > first.work.lease.as_ref().unwrap().fence);
}

#[tokio::test]
async fn handoffs_are_owner_scoped_and_intent_idempotent() {
    let store = InMemoryLaunchStore::default();
    let (_, launch) = admitted(&store).await;
    let handoff = ServiceConnectionHandoff {
        id: ServiceHandoffId("hnd_one".into()),
        launch_id: launch.id.clone(),
        owner: launch.owner.clone(),
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
    let created = store.create_handoff(handoff.clone()).await.unwrap();
    let mut replay = handoff.clone();
    replay.id = ServiceHandoffId::generate();
    assert_eq!(store.create_handoff(replay).await.unwrap(), created);
    assert!(store
        .handoff(&created.id, &SubjectId::new("stranger"))
        .await
        .unwrap()
        .is_none());

    let mut mismatch = handoff;
    mismatch.id = ServiceHandoffId::generate();
    mismatch.service_id = ServiceId::new("service-b");
    assert_eq!(
        store.create_handoff(mismatch).await,
        Err(LaunchStoreError::CommandConflict)
    );
}

#[test]
fn store_error_codes_are_stable() {
    assert_eq!(LaunchStoreError::NotFound.code(), "launch_not_found");
    assert_eq!(
        LaunchStoreError::CommandConflict.code(),
        "launch_idempotency_conflict"
    );
    assert_eq!(
        LaunchStoreError::InvalidCursor.code(),
        "invalid_launch_cursor"
    );
    assert_eq!(
        LaunchStoreError::Conflict {
            expected: 1,
            actual: 2
        }
        .code(),
        "launch_changed"
    );
}

#[test]
fn generated_launch_identifiers_keep_their_public_prefix() {
    assert!(LaunchId::generate().as_str().starts_with("lch_"));
}
