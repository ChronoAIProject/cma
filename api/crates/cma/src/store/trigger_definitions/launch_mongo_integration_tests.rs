//! Replica-set integration coverage for Launch runtime persistence.

use std::sync::Arc;

use layer2_core::ids::{LaunchId, ServiceHandoffId, ServiceId, SubjectId, TriggerId};
use layer2_core::sandbox::FirstMessageDelivery;
use layer2_core::trigger_domain::{
    BrokerHandoffState, Launch, LaunchAdmission, LaunchAdmissionResult, LaunchAuthorityClass,
    LaunchCommit, LaunchConfirmation, LaunchConsentSurface, LaunchEventKind, LaunchResolution,
    LaunchSeed, LaunchState, LaunchStore, LaunchStoreError, LaunchTrustManifest, PublicTrigger,
    RequirementsSnapshot, ReviewRequirements, ServiceConnectionHandoff, ServiceConnectionStore,
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
        created_at: NOW.into(),
    })
    .unwrap()
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
        layer2_core::trigger_domain::LaunchWorkStep::None,
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
async fn ready_admission_handoffs_indexes_and_corruption_are_strict() {
    let seed = seeded("launch_handoff").await;
    let ready_public = published(&seed, "ready_admission").await;
    let mut ready = launch(&ready_public, 'r', 's');
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
