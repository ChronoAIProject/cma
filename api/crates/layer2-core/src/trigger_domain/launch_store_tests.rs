use std::sync::Arc;

use crate::{
    AgentProfileId, LaunchId, SandboxProfileId, ServiceHandoffId, ServiceId, SubjectId,
    WorkspaceProfileId,
};

use super::*;
use crate::profiles::{
    AgentProfileRef, InMemoryProfileStore, SandboxProfileRef, WorkspaceProfileRef,
};

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
            inline: None,
        },
        reuse: ReusePolicy {
            sandbox: SandboxReusePolicy::ReuseExisting,
            workspace: WorkspaceReusePolicy::ReuseExisting,
            agent: AgentReusePolicy::AlwaysNew,
        },
        first_message: "Please review it.".into(),
        quick_messages: Vec::new(),
        consent: TriggerConsent {
            always_review: false,
        },
    }
}

fn store_fixture() -> (
    Arc<InMemoryTriggerStore>,
    InMemoryLaunchStore,
    PublicTrigger,
) {
    let profiles: Arc<dyn TriggerProfileResolver> = Arc::new(InMemoryProfileStore::default());
    let definitions = Arc::new(InMemoryTriggerStore::new(profiles));
    let owner = SubjectId::new("publisher");
    let mut trigger = Trigger::new(
        TriggerId("trg_one".into()),
        owner.clone(),
        content(),
        NOW.into(),
    )
    .unwrap();
    let revision = trigger.publish(NOW.into()).unwrap();
    let public = PublicTrigger {
        trigger_id: trigger.id.clone(),
        public_id: trigger.public_id.clone().unwrap(),
        owner,
        availability: TriggerAvailability::Published,
        revision,
    };
    definitions.seed_trigger(trigger).unwrap();
    let launches = InMemoryLaunchStore::new(definitions.clone());
    (definitions, launches, public)
}

fn launch(trigger: &PublicTrigger, key: char, fingerprint: char) -> Launch {
    launch_at(trigger, key, fingerprint, NOW)
}

fn launch_at(trigger: &PublicTrigger, key: char, fingerprint: char, created_at: &str) -> Launch {
    Launch::new(LaunchSeed {
        trigger_id: trigger.trigger_id.clone(),
        revision_digest: trigger.revision.digest.clone(),
        public_id: trigger.public_id.clone(),
        owner: SubjectId::new("visitor"),
        authority: LaunchAuthorityClass::SandboxCreator,
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
        trust: LaunchTrustManifest::none(),
        resolution: LaunchResolution::pending(),
        has_first_message: true,
        command_key_hash: digest(key),
        command_fingerprint: digest(fingerprint),
        created_at: created_at.into(),
    })
    .unwrap()
}

async fn admit_candidate(
    store: &InMemoryLaunchStore,
    trigger: &PublicTrigger,
    launch: Launch,
) -> Launch {
    store
        .admit_launch(LaunchAdmission {
            launch,
            trigger: trigger.clone(),
            events: vec![LaunchEventKind::Created],
        })
        .await
        .unwrap()
        .launch()
        .clone()
}

async fn admitted(store: &InMemoryLaunchStore, trigger: &PublicTrigger) -> Launch {
    let result = store
        .admit_launch(LaunchAdmission {
            launch: launch(trigger, 'a', 'b'),
            trigger: trigger.clone(),
            events: vec![LaunchEventKind::Created],
        })
        .await
        .unwrap();
    result.launch().clone()
}

async fn trigger_stats(
    definitions: &InMemoryTriggerStore,
    trigger: &PublicTrigger,
) -> TriggerStats {
    definitions
        .trigger(&trigger.trigger_id, &trigger.owner)
        .await
        .unwrap()
        .unwrap()
        .stats
}

#[tokio::test]
async fn admission_replays_exactly_and_counts_only_the_created_launch() {
    let (definitions, store, trigger) = store_fixture();
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
    assert_eq!(
        store
            .launch_by_command(
                &first.owner,
                &first.public_id,
                first.command_key_hash.as_str(),
            )
            .await
            .unwrap()
            .unwrap()
            .id,
        first.id
    );
    assert_eq!(
        trigger_stats(&definitions, &trigger).await.launches_total,
        1
    );

    definitions
        .retire_trigger(&trigger.trigger_id, &trigger.owner, LATER.into())
        .await
        .unwrap();
    let replay = store
        .admit_launch(LaunchAdmission {
            launch: launch(&trigger, 'a', 'b'),
            trigger: trigger.clone(),
            events: vec![LaunchEventKind::Created],
        })
        .await
        .unwrap();
    assert!(!replay.was_created());

    assert_eq!(
        store
            .admit_launch(LaunchAdmission {
                launch: launch(&trigger, 'a', 'c'),
                trigger: trigger.clone(),
                events: vec![LaunchEventKind::Created],
            })
            .await,
        Err(LaunchStoreError::CommandConflict)
    );
    assert_eq!(
        store
            .admit_launch(LaunchAdmission {
                launch: launch(&trigger, 'd', 'e'),
                trigger: trigger.clone(),
                events: vec![LaunchEventKind::Created],
            })
            .await,
        Err(LaunchStoreError::TriggerRetired)
    );
    assert_eq!(
        trigger_stats(&definitions, &trigger).await.launches_total,
        1
    );
}

#[tokio::test]
async fn concurrent_same_command_admission_counts_once() {
    let (definitions, store, trigger) = store_fixture();
    let left = store.admit_launch(LaunchAdmission {
        launch: launch(&trigger, 'd', 'e'),
        trigger: trigger.clone(),
        events: vec![LaunchEventKind::Created],
    });
    let right = store.admit_launch(LaunchAdmission {
        launch: launch(&trigger, 'd', 'e'),
        trigger: trigger.clone(),
        events: vec![LaunchEventKind::Created],
    });
    let (left, right) = tokio::join!(left, right);
    let results = [left.unwrap(), right.unwrap()];
    assert_eq!(
        results.iter().filter(|result| result.was_created()).count(),
        1
    );
    assert_eq!(results[0].launch().id, results[1].launch().id);
    assert_eq!(
        trigger_stats(&definitions, &trigger).await,
        TriggerStats {
            launches_total: 1,
            agents_ready_total: 0,
        }
    );
}

#[tokio::test]
async fn launch_pages_are_owner_scoped_filter_bound_and_deterministic() {
    let (_, store, trigger) = store_fixture();
    let oldest = admit_candidate(&store, &trigger, launch_at(&trigger, '1', 'a', NOW)).await;
    let middle = admit_candidate(&store, &trigger, launch_at(&trigger, '2', 'b', LATER)).await;
    let newest = admit_candidate(&store, &trigger, launch_at(&trigger, '3', 'c', LATEST)).await;
    let mut foreign = launch_at(&trigger, '4', 'd', LATEST);
    foreign.owner = SubjectId::new("somebody-else");
    let foreign = admit_candidate(&store, &trigger, foreign).await;

    let first = store
        .list_launches(
            &oldest.owner,
            &LaunchPageRequest {
                limit: 2,
                ..LaunchPageRequest::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        first
            .items
            .iter()
            .map(|launch| &launch.id)
            .collect::<Vec<_>>(),
        [&newest.id, &middle.id]
    );
    let second = store
        .list_launches(
            &oldest.owner,
            &LaunchPageRequest {
                limit: 2,
                cursor: first.next.clone(),
                ..LaunchPageRequest::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(second.items, vec![oldest.clone()]);
    assert!(second.next.is_none());
    assert_eq!(
        store
            .list_launches(
                &foreign.owner,
                &LaunchPageRequest {
                    limit: 2,
                    ..LaunchPageRequest::default()
                },
            )
            .await
            .unwrap()
            .items,
        vec![foreign]
    );

    let changed_filter = LaunchPageRequest {
        limit: 2,
        cursor: first.next,
        trigger: Some(trigger.trigger_id.clone()),
        state: None,
    };
    assert_eq!(
        store.list_launches(&oldest.owner, &changed_filter).await,
        Err(LaunchStoreError::InvalidCursor)
    );
    assert_eq!(
        store
            .list_launches(
                &oldest.owner,
                &LaunchPageRequest {
                    limit: 0,
                    ..LaunchPageRequest::default()
                },
            )
            .await,
        Err(LaunchStoreError::InvalidCursor)
    );
}

#[tokio::test]
async fn placement_stats_include_unattributed_launches_without_crossing_triggers() {
    let (_, store, trigger) = store_fixture();
    let placement = PlacementId("plc_readme".into());
    for (key, fingerprint, placement_id) in [
        ('1', 'a', Some(placement.clone())),
        ('2', 'b', Some(placement.clone())),
        ('3', 'c', None),
    ] {
        let mut candidate = launch(&trigger, key, fingerprint);
        candidate.placement_id = placement_id;
        admit_candidate(&store, &trigger, candidate).await;
    }
    assert_eq!(
        store.placement_stats(&trigger.trigger_id).await.unwrap(),
        vec![
            LaunchPlacementStats {
                placement_id: None,
                launches_total: 1,
                agents_ready_total: 0,
            },
            LaunchPlacementStats {
                placement_id: Some(placement),
                launches_total: 2,
                agents_ready_total: 0,
            },
        ]
    );
    assert!(store
        .placement_stats(&TriggerId("trg_other".into()))
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn commits_are_revision_fenced_and_append_contiguous_events() {
    let (_, store, trigger) = store_fixture();
    let mut launch = admitted(&store, &trigger).await;
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
    let (_, store, trigger) = store_fixture();
    let mut launch = admitted(&store, &trigger).await;
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
    let first_fence = first.work.lease.as_ref().unwrap().fence;
    let mut executing = first.clone();
    executing
        .work
        .prepare_effect(
            "worker-a",
            first_fence,
            LaunchEffectKind::CreateSandbox,
            LaunchEffectKind::CreateSandbox.intent_key(&launch.id),
            NOW.into(),
        )
        .unwrap();
    executing
        .work
        .mark_effect_executing("worker-a", first_fence, NOW.into())
        .unwrap();
    executing.revision += 1;
    store
        .commit_launch(LaunchCommit {
            expected_revision: first.revision,
            launch: executing,
            event_at: NOW.into(),
            events: vec![],
        })
        .await
        .unwrap();
    let second = store
        .claim_launch(&launch.id, "worker-b", LATER.into(), LATEST.into())
        .await
        .unwrap()
        .unwrap();
    assert!(second.work.lease.as_ref().unwrap().fence > first_fence);
    assert_eq!(
        second.work.effect.as_ref().unwrap().state,
        LaunchEffectState::RecoveryRequired
    );
}

#[tokio::test]
async fn wakeable_blocks_are_due_and_fenced_while_recovery_stays_out_of_band() {
    let (_, store, trigger) = store_fixture();
    let service = ServiceId::new("github");
    let reasons = [
        ('c', 'd', LaunchBlockedReason::OrchestrationPaused),
        ('e', 'f', LaunchBlockedReason::SandboxPausedReviving),
        (
            '1',
            '2',
            LaunchBlockedReason::ConnectionRequired {
                service: service.clone(),
            },
        ),
    ];
    let mut wakeable = Vec::new();

    for (key, fingerprint, reason) in reasons {
        let mut candidate = launch(&trigger, key, fingerprint);
        if matches!(reason, LaunchBlockedReason::ConnectionRequired { .. }) {
            candidate.requirements.services.push(ServiceRequirement {
                service: service.clone(),
                state: ServiceRequirementState::Satisfied,
            });
            candidate.validate().unwrap();
        }
        let mut current = store
            .admit_launch(LaunchAdmission {
                launch: candidate,
                trigger: trigger.clone(),
                events: vec![LaunchEventKind::Created],
            })
            .await
            .unwrap()
            .launch()
            .clone();
        let expected = current.revision;
        current
            .start(LaunchConfirmation {
                surface: LaunchConsentSurface::Landing,
                trust_confirmed_digest: None,
                acknowledgements: vec![],
                confirmed_at: NOW.into(),
            })
            .unwrap();
        current = store
            .commit_launch(LaunchCommit {
                expected_revision: expected,
                launch: current,
                event_at: NOW.into(),
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
        let claimed = store
            .claim_launch(&current.id, "worker-a", NOW.into(), LATER.into())
            .await
            .unwrap()
            .unwrap();
        let first_fence = claimed.work.lease.as_ref().unwrap().fence;
        let mut blocked = claimed.clone();
        let refresh = matches!(reason, LaunchBlockedReason::ConnectionRequired { .. }).then(|| {
            LaunchRequirementRefresh {
                service: service.clone(),
                state: ServiceRequirementState::Missing,
                checked_at: NOW.into(),
            }
        });
        let events = blocked
            .block_worker_launch(
                "worker-a",
                first_fence,
                reason.clone(),
                refresh,
                LATER.into(),
                NOW.into(),
            )
            .unwrap();
        let blocked = store
            .commit_launch(LaunchCommit {
                expected_revision: claimed.revision,
                launch: blocked,
                event_at: NOW.into(),
                events,
            })
            .await
            .unwrap();
        assert_eq!(blocked.blocked_reason, Some(reason));
        assert!(blocked.work.lease.is_none());
        wakeable.push((blocked.id, first_fence));
    }

    assert!(store.due_launches(NOW, 10).await.unwrap().is_empty());
    let due = store.due_launches(LATER, 10).await.unwrap();
    assert_eq!(due.len(), wakeable.len());
    assert!(wakeable.iter().all(|(id, _)| due.contains(id)));

    let (reblocked_id, first_fence) = &wakeable[0];
    let mut claimed = store
        .claim_launch(reblocked_id, "worker-b", LATER.into(), LATEST.into())
        .await
        .unwrap()
        .unwrap();
    let second_fence = claimed.work.lease.as_ref().unwrap().fence;
    assert!(second_fence > *first_fence);
    let before = claimed.clone();
    assert!(claimed
        .block_worker_launch(
            "worker-a",
            *first_fence,
            LaunchBlockedReason::OrchestrationPaused,
            None,
            LATEST.into(),
            LATER.into(),
        )
        .is_err());
    assert_eq!(claimed, before);
    let events = claimed
        .block_worker_launch(
            "worker-b",
            second_fence,
            LaunchBlockedReason::SandboxPausedReviving,
            None,
            LATEST.into(),
            LATER.into(),
        )
        .unwrap();
    store
        .commit_launch(LaunchCommit {
            expected_revision: before.revision,
            launch: claimed,
            event_at: LATER.into(),
            events,
        })
        .await
        .unwrap();
    assert!(!store
        .due_launches(LATER, 10)
        .await
        .unwrap()
        .contains(reblocked_id));
    assert!(store
        .due_launches(LATEST, 10)
        .await
        .unwrap()
        .contains(reblocked_id));

    let mut recovery = store
        .admit_launch(LaunchAdmission {
            launch: launch(&trigger, '3', '4'),
            trigger: trigger.clone(),
            events: vec![LaunchEventKind::Created],
        })
        .await
        .unwrap()
        .launch()
        .clone();
    let expected = recovery.revision;
    recovery
        .start(LaunchConfirmation {
            surface: LaunchConsentSurface::Landing,
            trust_confirmed_digest: None,
            acknowledgements: vec![],
            confirmed_at: NOW.into(),
        })
        .unwrap();
    recovery = store
        .commit_launch(LaunchCommit {
            expected_revision: expected,
            launch: recovery,
            event_at: NOW.into(),
            events: vec![LaunchEventKind::ConfirmationRecorded],
        })
        .await
        .unwrap();
    recovery = store
        .claim_launch(&recovery.id, "recovery-worker", NOW.into(), LATER.into())
        .await
        .unwrap()
        .unwrap();
    let recovery_fence = recovery.work.lease.as_ref().unwrap().fence;
    let expected = recovery.revision;
    let events = recovery
        .prepare_worker_effect(
            "recovery-worker",
            recovery_fence,
            LaunchEffectKind::CreateSandbox,
            NOW.into(),
        )
        .unwrap();
    recovery = store
        .commit_launch(LaunchCommit {
            expected_revision: expected,
            launch: recovery,
            event_at: NOW.into(),
            events,
        })
        .await
        .unwrap();
    let expected = recovery.revision;
    recovery
        .mark_worker_effect_executing("recovery-worker", recovery_fence, NOW.into())
        .unwrap();
    recovery = store
        .commit_launch(LaunchCommit {
            expected_revision: expected,
            launch: recovery,
            event_at: NOW.into(),
            events: vec![],
        })
        .await
        .unwrap();
    let expected = recovery.revision;
    let events = recovery
        .block_worker_recovery("recovery-worker", recovery_fence, NOW.into())
        .unwrap();
    recovery = store
        .commit_launch(LaunchCommit {
            expected_revision: expected,
            launch: recovery,
            event_at: NOW.into(),
            events,
        })
        .await
        .unwrap();
    assert_eq!(
        recovery.blocked_reason,
        Some(LaunchBlockedReason::RecoveryRequired)
    );
    assert!(!store
        .due_launches(LATEST, 10)
        .await
        .unwrap()
        .contains(&recovery.id));
    assert!(store
        .claim_launch(
            &recovery.id,
            "normal-worker",
            LATEST.into(),
            "2026-09-05T00:03:00Z".into(),
        )
        .await
        .unwrap()
        .is_none());

    let owner = SubjectId::new("visitor");
    for id in wakeable
        .into_iter()
        .map(|(id, _)| id)
        .chain(std::iter::once(recovery.id))
    {
        let current = store.launch(&id, &owner).await.unwrap().unwrap();
        let events = store.launch_events(&id, &owner, 0, 100).await.unwrap();
        assert_eq!(events.len() as u64, current.event_head);
        assert_eq!(
            events.iter().map(|event| event.seq).collect::<Vec<_>>(),
            (1..=current.event_head).collect::<Vec<_>>()
        );
    }
}

#[tokio::test]
async fn handoffs_are_owner_scoped_and_intent_idempotent() {
    let (_, store, trigger) = store_fixture();
    let launch = admitted(&store, &trigger).await;
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
    assert_eq!(
        store
            .handoff_by_intent(&created.owner, &created.launch_id, &created.intent_id)
            .await
            .unwrap(),
        Some(created.clone())
    );
    assert!(store
        .handoff_by_intent(
            &SubjectId::new("stranger"),
            &created.launch_id,
            &created.intent_id,
        )
        .await
        .unwrap()
        .is_none());
    assert!(store
        .handoff_by_intent(&created.owner, &created.launch_id, "different-intent")
        .await
        .unwrap()
        .is_none());
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
