//! Replica-set integration coverage for Launch runtime persistence.

use std::sync::Arc;

use layer2_core::ids::{LaunchId, PlacementId, ServiceHandoffId, ServiceId, SubjectId, TriggerId};
use layer2_core::sandbox::FirstMessageDelivery;
use layer2_core::trigger_domain::{
    BrokerHandoffState, Launch, LaunchAdmission, LaunchAdmissionResult, LaunchAuthorityClass,
    LaunchBlockedReason, LaunchCommit, LaunchConfirmation, LaunchConsentSurface, LaunchEventKind,
    LaunchPageRequest, LaunchResolution, LaunchSeed, LaunchState, LaunchStore, LaunchStoreError,
    LaunchTrustManifest, PublicTrigger, RequirementsSnapshot, ReviewRequirements,
    ServiceConnectionHandoff, ServiceConnectionStore, ServiceRequirement, ServiceRequirementState,
    TriggerStats, TriggerStore,
};
use mongodb::bson::{doc, Document};

use layer2_core::StoreError;

use super::integration_tests::{seeded, trigger, Seeded};
use super::launches::{LAUNCHES, LAUNCH_EVENTS, SERVICE_HANDOFFS};
use super::MongoTriggerDefinitionStore;

const NOW: &str = "2026-09-05T04:00:00Z";
const LATER: &str = "2026-09-05T04:01:00Z";
const LATEST: &str = "2026-09-05T04:02:00Z";

mod initiation;
#[path = "launch_observation_mongo_tests.rs"]
mod observation_tests;
mod platform_spend;
mod visitor_quota;

fn digest(byte: char) -> String {
    byte.to_string().repeat(64)
}

async fn published(seed: &Seeded, suffix: &str) -> PublicTrigger {
    let id = TriggerId(format!("trg_launch_{suffix}"));
    seed.store
        .create_trigger(trigger(id.as_str(), &seed.owner, &seed.target))
        .await
        .unwrap();
    seed.store
        .publish_trigger(&id, &seed.owner, 1, NOW.into())
        .await
        .unwrap()
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

async fn admit_candidate(seed: &Seeded, trigger: &PublicTrigger, launch: Launch) -> Launch {
    seed.store
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

async fn admit(seed: &Seeded, trigger: &PublicTrigger) -> Launch {
    seed.store
        .admit_launch(LaunchAdmission {
            launch: launch(trigger, 'a', 'b'),
            trigger: trigger.clone(),
            events: vec![LaunchEventKind::Created],
        })
        .await
        .unwrap()
        .launch()
        .clone()
}

async fn stats(seed: &Seeded, trigger: &PublicTrigger) -> TriggerStats {
    seed.store
        .trigger(&trigger.trigger_id, &trigger.owner)
        .await
        .unwrap()
        .unwrap()
        .stats
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn landing_consent_survives_connection_wait_and_store_restart_without_reconfirmation() {
    let seed = seeded("launch_pending_consent").await;
    let mut definition = trigger("trg_pending_consent", &seed.owner, &seed.target);
    definition.draft.consent.always_review = false;
    seed.store.create_trigger(definition).await.unwrap();
    let public = seed
        .store
        .publish_trigger(
            &TriggerId("trg_pending_consent".into()),
            &seed.owner,
            1,
            NOW.into(),
        )
        .await
        .unwrap();
    let mut candidate = launch(&public, 'a', 'b');
    candidate.requirements.services = vec![ServiceRequirement {
        service: ServiceId::new("api-github"),
        state: ServiceRequirementState::Missing,
    }];
    candidate.review =
        ReviewRequirements::build(&candidate.requirements, None, false, false).unwrap();
    candidate
        .confirm_landing_start(LaunchConfirmation {
            surface: LaunchConsentSurface::Landing,
            trust_confirmed_digest: None,
            acknowledgements: Vec::new(),
            confirmed_at: NOW.into(),
        })
        .unwrap();
    let stored = seed
        .store
        .admit_launch(LaunchAdmission {
            launch: candidate.clone(),
            trigger: public.clone(),
            events: vec![
                LaunchEventKind::Created,
                LaunchEventKind::ConfirmationRecorded,
            ],
        })
        .await
        .unwrap()
        .launch()
        .clone();
    assert_eq!(stored.state, LaunchState::Draft);
    assert!(stored.started_at.is_none());
    let reopened =
        MongoTriggerDefinitionStore::open(&seed.client, &seed.database, Arc::clone(&seed.profiles))
            .await
            .unwrap();
    let mut observed = reopened
        .launch(&stored.id, &stored.owner)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(observed, stored);
    assert!(reopened.due_launches(LATER, 10).await.unwrap().is_empty());
    for clear in [false, true] {
        let mut changed = observed.clone();
        if clear {
            changed.confirmations.clear();
        } else {
            changed.confirmations[0]
                .acknowledgements
                .push("changed-after-click".into());
        }
        changed.revision += 1;
        assert!(matches!(
            reopened
                .commit_launch(LaunchCommit {
                    expected_revision: observed.revision,
                    launch: changed,
                    event_at: LATER.into(),
                    events: Vec::new(),
                })
                .await,
            Err(LaunchStoreError::Corrupt(_))
        ));
    }
    let expected_revision = observed.revision;
    let requirements = RequirementsSnapshot {
        services: vec![ServiceRequirement {
            service: ServiceId::new("api-github"),
            state: ServiceRequirementState::Satisfied,
        }],
        checked_at: LATER.into(),
    };
    let review = ReviewRequirements::build(&requirements, None, false, false).unwrap();
    observed
        .refresh_review(requirements, review, LATER.into())
        .unwrap();
    observed = reopened
        .commit_launch(LaunchCommit {
            expected_revision,
            launch: observed,
            event_at: LATER.into(),
            events: vec![LaunchEventKind::RequirementChanged {
                service: ServiceId::new("api-github"),
                state: ServiceRequirementState::Satisfied,
            }],
        })
        .await
        .unwrap();
    let expected_revision = observed.revision;
    observed.start_confirmed(LATEST.into()).unwrap();
    let started = reopened
        .commit_launch(LaunchCommit {
            expected_revision,
            launch: observed,
            event_at: LATEST.into(),
            events: vec![LaunchEventKind::StateChanged {
                from: LaunchState::Draft,
                to: LaunchState::Resolving,
            }],
        })
        .await
        .unwrap();
    assert_eq!(started.confirmations, stored.confirmations);
    assert_eq!(started.started_at.as_deref(), Some(LATEST));
    assert_eq!(started.confirmations[0].confirmed_at, NOW);
    let events = reopened
        .launch_events(&stored.id, &stored.owner, 0, 10)
        .await
        .unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == LaunchEventKind::ConfirmationRecorded)
            .count(),
        1
    );
    seed.client.database(&seed.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn admission_replays_conflicts_counts_once_and_survives_restart() {
    let seed = seeded("launch_admission").await;
    let public = published(&seed, "admission").await;
    let intended = launch(&public, 'a', 'b');
    let left = seed.store.admit_launch(LaunchAdmission {
        launch: intended.clone(),
        trigger: public.clone(),
        events: vec![LaunchEventKind::Created],
    });
    let right = seed.store.admit_launch(LaunchAdmission {
        launch: intended.clone(),
        trigger: public.clone(),
        events: vec![LaunchEventKind::Created],
    });
    let (left, right) = tokio::join!(left, right);
    let results = [left.unwrap(), right.unwrap()];
    assert_eq!(
        results.iter().filter(|result| result.was_created()).count(),
        1
    );
    assert_eq!(results[0].launch(), results[1].launch());
    assert_eq!(stats(&seed, &public).await.launches_total, 1);

    let mut conflict = intended.clone();
    conflict.id = LaunchId::generate();
    conflict.command_fingerprint = digest('c');
    assert_eq!(
        seed.store
            .admit_launch(LaunchAdmission {
                launch: conflict,
                trigger: public.clone(),
                events: vec![LaunchEventKind::Created],
            })
            .await,
        Err(LaunchStoreError::CommandConflict)
    );
    seed.store
        .retire_trigger(&public.trigger_id, &public.owner, LATER.into())
        .await
        .unwrap();
    assert!(matches!(
        seed.store
            .admit_launch(LaunchAdmission {
                launch: intended.clone(),
                trigger: public.clone(),
                events: vec![LaunchEventKind::Created],
            })
            .await
            .unwrap(),
        LaunchAdmissionResult::Existing(_)
    ));

    let restarted =
        MongoTriggerDefinitionStore::open(&seed.client, &seed.database, Arc::clone(&seed.profiles))
            .await
            .unwrap();
    assert_eq!(
        restarted
            .launch(&intended.id, &intended.owner)
            .await
            .unwrap()
            .unwrap()
            .id,
        intended.id
    );
    seed.client.database(&seed.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn commits_append_gap_free_events_claim_due_work_and_count_ready_once() {
    let seed = seeded("launch_commit").await;
    let public = published(&seed, "commit").await;
    let mut current = admit(&seed, &public).await;
    let expected = current.revision;
    current
        .start(LaunchConfirmation {
            surface: LaunchConsentSurface::Landing,
            trust_confirmed_digest: None,
            acknowledgements: vec![],
            confirmed_at: NOW.into(),
        })
        .unwrap();
    current = seed
        .store
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
    assert_eq!(
        seed.store
            .launch_events(&current.id, &current.owner, 0, 100)
            .await
            .unwrap()
            .iter()
            .map(|event| event.seq)
            .collect::<Vec<_>>(),
        [1, 2, 3]
    );
    assert_eq!(
        seed.store.due_launches(NOW, 10).await.unwrap(),
        [current.id.clone()]
    );
    let claimed = seed
        .store
        .claim_launch(&current.id, "worker-a", NOW.into(), LATER.into())
        .await
        .unwrap()
        .unwrap();
    assert!(seed
        .store
        .claim_launch(&current.id, "worker-b", NOW.into(), LATER.into())
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        seed.store
            .commit_launch(LaunchCommit {
                expected_revision: expected,
                launch: claimed.clone(),
                event_at: LATER.into(),
                events: vec![],
            })
            .await,
        Err(LaunchStoreError::Conflict {
            expected,
            actual: claimed.revision,
        })
    );

    let mut ready = claimed.clone();
    ready.work.release_lease();
    ready.state = LaunchState::Ready;
    ready.first_message = FirstMessageDelivery::Completed;
    ready.ready_at = Some(LATER.into());
    ready.updated_at = LATER.into();
    ready.revision += 1;
    for step in &mut ready.steps {
        step.status = layer2_core::trigger_domain::LaunchStepStatus::Completed;
        step.started_at.get_or_insert_with(|| LATER.into());
        step.completed_at.get_or_insert_with(|| LATER.into());
    }
    ready.work = layer2_core::trigger_domain::LaunchWorkState::new(
        layer2_core::trigger_domain::LaunchWorkStep::Finish,
        LATER.into(),
    );
    let ready = seed
        .store
        .commit_launch(LaunchCommit {
            expected_revision: claimed.revision,
            launch: ready,
            event_at: LATER.into(),
            events: vec![],
        })
        .await
        .unwrap();
    assert_eq!(stats(&seed, &public).await.agents_ready_total, 1);
    let mut completed = ready.clone();
    completed
        .transition(LaunchState::Completed, LATEST.into())
        .unwrap();
    seed.store
        .commit_launch(LaunchCommit {
            expected_revision: ready.revision,
            launch: completed,
            event_at: LATEST.into(),
            events: vec![LaunchEventKind::Completed],
        })
        .await
        .unwrap();
    assert_eq!(stats(&seed, &public).await.agents_ready_total, 1);
    seed.client.database(&seed.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn listing_and_placement_stats_are_bounded_owned_and_filter_bound() {
    let seed = seeded("launch_listing").await;
    let public = published(&seed, "listing").await;
    let placement = PlacementId("plc_readme".into());
    let mut oldest = launch_at(&public, '1', 'a', NOW);
    oldest.id = LaunchId("lch_page_oldest".into());
    oldest.placement_id = Some(placement.clone());
    let oldest = admit_candidate(&seed, &public, oldest).await;
    let mut middle = launch_at(&public, '2', 'b', LATER);
    middle.id = LaunchId("lch_page_middle".into());
    middle.placement_id = Some(placement.clone());
    let middle = admit_candidate(&seed, &public, middle).await;
    let mut newest = launch_at(&public, '3', 'c', LATEST);
    newest.id = LaunchId("lch_page_newest".into());
    let newest = admit_candidate(&seed, &public, newest).await;

    let request = LaunchPageRequest {
        limit: 2,
        cursor: None,
        trigger: Some(public.trigger_id.clone()),
        state: Some(LaunchState::Draft),
    };
    let first = seed
        .store
        .list_launches(&oldest.owner, &request)
        .await
        .unwrap();
    assert_eq!(first.items, vec![newest, middle]);
    let second = seed
        .store
        .list_launches(
            &oldest.owner,
            &LaunchPageRequest {
                cursor: first.next.clone(),
                ..request.clone()
            },
        )
        .await
        .unwrap();
    assert_eq!(second.items, vec![oldest.clone()]);
    assert!(second.next.is_none());
    assert!(seed
        .store
        .list_launches(&SubjectId::new("other"), &request)
        .await
        .unwrap()
        .items
        .is_empty());
    assert_eq!(
        seed.store
            .list_launches(
                &oldest.owner,
                &LaunchPageRequest {
                    cursor: first.next,
                    state: None,
                    ..request
                },
            )
            .await,
        Err(LaunchStoreError::InvalidCursor)
    );

    let placement_stats = seed
        .store
        .placement_stats(&public.trigger_id)
        .await
        .unwrap();
    assert_eq!(placement_stats.len(), 2);
    assert_eq!(
        placement_stats
            .iter()
            .find(|stats| stats.placement_id.is_none())
            .map(|stats| stats.launches_total),
        Some(1)
    );
    assert_eq!(
        placement_stats
            .iter()
            .find(|stats| stats.placement_id.as_ref() == Some(&placement))
            .map(|stats| stats.launches_total),
        Some(2)
    );
    seed.client.database(&seed.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn wakeable_blocked_launches_are_due_and_claimable_in_mongo() {
    let seed = seeded("launch_wakeable_blocks").await;
    let public = published(&seed, "wakeable_blocks").await;
    let service = ServiceId::new("github");
    let reasons = [
        ('1', 'a', LaunchBlockedReason::OrchestrationPaused),
        ('2', 'b', LaunchBlockedReason::SandboxPausedReviving),
        (
            '3',
            'c',
            LaunchBlockedReason::ConnectionRequired {
                service: service.clone(),
            },
        ),
    ];
    let mut ids = Vec::new();
    for (key, fingerprint, reason) in reasons {
        let mut candidate = launch(&public, key, fingerprint);
        if matches!(reason, LaunchBlockedReason::ConnectionRequired { .. }) {
            candidate.requirements.services.push(ServiceRequirement {
                service: service.clone(),
                state: ServiceRequirementState::Satisfied,
            });
        }
        let mut current = admit_candidate(&seed, &public, candidate).await;
        let expected = current.revision;
        current
            .start(LaunchConfirmation {
                surface: LaunchConsentSurface::Landing,
                trust_confirmed_digest: None,
                acknowledgements: vec![],
                confirmed_at: NOW.into(),
            })
            .unwrap();
        current = seed
            .store
            .commit_launch(LaunchCommit {
                expected_revision: expected,
                launch: current,
                event_at: NOW.into(),
                events: vec![LaunchEventKind::ConfirmationRecorded],
            })
            .await
            .unwrap();
        let claimed = seed
            .store
            .claim_launch(&current.id, "worker-a", NOW.into(), LATER.into())
            .await
            .unwrap()
            .unwrap();
        let fence = claimed.work.lease.as_ref().unwrap().fence;
        let mut blocked = claimed.clone();
        let refresh = matches!(reason, LaunchBlockedReason::ConnectionRequired { .. }).then(|| {
            layer2_core::trigger_domain::LaunchRequirementRefresh {
                service: service.clone(),
                state: ServiceRequirementState::Missing,
                checked_at: NOW.into(),
            }
        });
        let events = blocked
            .block_worker_launch("worker-a", fence, reason, refresh, LATER.into(), NOW.into())
            .unwrap();
        let blocked = seed
            .store
            .commit_launch(LaunchCommit {
                expected_revision: claimed.revision,
                launch: blocked,
                event_at: NOW.into(),
                events,
            })
            .await
            .unwrap();
        ids.push(blocked.id);
    }

    assert!(seed.store.due_launches(NOW, 10).await.unwrap().is_empty());
    let due = seed.store.due_launches(LATER, 10).await.unwrap();
    assert_eq!(due.len(), 3);
    assert!(ids.iter().all(|id| due.contains(id)));
    for id in ids {
        assert!(seed
            .store
            .claim_launch(&id, "worker-b", LATER.into(), LATEST.into())
            .await
            .unwrap()
            .is_some());
    }
    seed.client.database(&seed.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn ready_admission_handoffs_indexes_and_corruption_are_strict() {
    let seed = seeded("launch_handoff").await;
    let ready_public = published(&seed, "ready_admission").await;
    let mut ready = launch(&ready_public, 'c', 'd');
    ready
        .start(LaunchConfirmation {
            surface: LaunchConsentSurface::Landing,
            trust_confirmed_digest: None,
            acknowledgements: vec![],
            confirmed_at: NOW.into(),
        })
        .unwrap();
    ready.transition(LaunchState::Ready, LATER.into()).unwrap();
    ready.first_message = FirstMessageDelivery::Completed;
    for step in &mut ready.steps {
        step.status = layer2_core::trigger_domain::LaunchStepStatus::Completed;
        step.started_at.get_or_insert_with(|| LATER.into());
        step.completed_at.get_or_insert_with(|| LATER.into());
    }
    ready.validate().unwrap();
    assert!(seed
        .store
        .admit_launch(LaunchAdmission {
            launch: ready,
            trigger: ready_public.clone(),
            events: vec![LaunchEventKind::Created],
        })
        .await
        .unwrap()
        .was_created());
    assert_eq!(
        stats(&seed, &ready_public).await,
        TriggerStats {
            launches_total: 1,
            agents_ready_total: 1,
        }
    );

    let public = published(&seed, "handoff").await;
    let launch = admit(&seed, &public).await;
    let handoff = ServiceConnectionHandoff {
        id: ServiceHandoffId("hnd_launch_test".into()),
        launch_id: launch.id.clone(),
        owner: launch.owner.clone(),
        service_id: ServiceId::new("github"),
        intent_id: "intent-one".into(),
        hosted_url: "https://connection.example/one".into(),
        expires_at: LATEST.into(),
        state: BrokerHandoffState::Pending,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        external_id: "external-one".into(),
        revision: 1,
    };
    let created = seed.store.create_handoff(handoff.clone()).await.unwrap();
    let mut replay = handoff.clone();
    replay.id = ServiceHandoffId::generate();
    assert_eq!(seed.store.create_handoff(replay).await.unwrap(), created);
    assert!(seed
        .store
        .handoff(&created.id, &SubjectId::new("other"))
        .await
        .unwrap()
        .is_none());
    let mut changed = created.clone();
    changed.state = BrokerHandoffState::Satisfied;
    changed.updated_at = LATER.into();
    changed.revision = 2;
    assert_eq!(
        seed.store.save_handoff(1, changed.clone()).await.unwrap(),
        changed
    );
    assert!(matches!(
        seed.store.save_handoff(1, changed).await,
        Err(LaunchStoreError::Conflict { actual: 2, .. })
    ));

    let database = seed.client.database(&seed.database);
    for (collection, index) in [
        (LAUNCHES, "trigger_launches_owner_command"),
        (LAUNCH_EVENTS, "trigger_launch_events_launch_sequence"),
        (SERVICE_HANDOFFS, "service_connection_handoffs_owner_intent"),
    ] {
        assert!(database
            .collection::<Document>(collection)
            .list_index_names()
            .await
            .unwrap()
            .contains(&index.to_string()));
    }
    database
        .collection::<Document>(LAUNCHES)
        .update_one(
            doc! { "_id": launch.id.as_str() },
            doc! { "$set": { "work_ready_at": LATEST } },
        )
        .await
        .unwrap();
    assert!(matches!(
        seed.store.launch(&launch.id, &launch.owner).await,
        Err(LaunchStoreError::Corrupt(_))
    ));
    database
        .collection::<Document>(SERVICE_HANDOFFS)
        .insert_one(doc! {
            "_id": "hnd_future",
            "document_schema_version": 2_i64,
        })
        .await
        .unwrap();
    assert!(matches!(
        MongoTriggerDefinitionStore::open(
            &seed.client,
            &seed.database,
            Arc::clone(&seed.profiles),
        )
        .await,
        Err(StoreError::Malformed(_))
    ));
    database.drop().await.unwrap();
}
