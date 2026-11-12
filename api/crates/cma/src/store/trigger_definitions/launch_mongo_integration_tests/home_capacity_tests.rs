//! Fresh Launches reserve fleet capacity before a worker creates their homes.
use super::*;
use crate::store::sandbox_mongo::MongoSandboxStore;
use layer2_core::admission::{AdmissionCeilings, AdmissionCeilingsStore, ExistingReserve};
use layer2_core::sandbox::store::{
    SandboxAdmission, SandboxAdmissionName, SandboxAdmissionStore, SandboxStoreError,
};
use layer2_core::sandbox::{Sandbox, SandboxOrigin};
use layer2_core::trigger_domain::ResolvedResource;

async fn configure(seed: &Seeded, ceilings: AdmissionCeilings) {
    crate::store::MongoRegistrationCapacityStore::open(&seed.client, &seed.database, true)
        .await
        .unwrap();
    let settings = crate::store::admission_ceilings::MongoAdmissionCeilingsStore::open(
        &seed.client,
        &seed.database,
        0,
    )
    .await
    .unwrap();
    let revision = settings.read().await.unwrap().revision;
    settings
        .replace(
            revision,
            &SubjectId::new("operator"),
            "configure Launch capacity",
            ceilings,
            100 + revision,
        )
        .await
        .unwrap();
}

async fn resources(seed: &Seeded) -> MongoSandboxStore {
    MongoSandboxStore::open(&seed.client, &seed.database)
        .await
        .unwrap()
}

fn request(trigger: &PublicTrigger, index: usize) -> LaunchAdmission {
    let mut candidate = launch(trigger, 'a', 'b');
    candidate.owner = SubjectId::new(format!("launch-capacity-{index}"));
    LaunchAdmission {
        launch: candidate,
        trigger: trigger.clone(),
        events: vec![LaunchEventKind::Created],
    }
}

async fn pending(seed: &Seeded) -> u64 {
    let row = seed
        .client
        .database(&seed.database)
        .collection::<Document>("sandbox_admissions")
        .find_one(crate::store::launch_home_capacity::fence_key())
        .await
        .unwrap();
    crate::store::launch_home_capacity::pending_homes(row.as_ref()).unwrap()
}

async fn receipt(seed: &Seeded, launch: &Launch) -> Document {
    seed.client
        .database(&seed.database)
        .collection::<Document>("sandbox_admissions")
        .find_one(crate::store::launch_home_capacity::receipt_key(
            launch.id.as_str(),
        ))
        .await
        .unwrap()
        .unwrap()
}

fn home(seed: &Seeded, launch: &Launch, id: &str, fresh: bool) -> SandboxAdmission {
    let mut sandbox: Sandbox =
        crate::store::sandbox_admission_mongo_integration_tests::sandbox(id, launch.id.as_str());
    sandbox.owner = launch.owner.clone();
    sandbox.profile.id = seed.target.sandbox_profile.id.clone();
    sandbox.name = "Launch capacity".into();
    if fresh {
        sandbox.origin = SandboxOrigin::Manual;
    }
    SandboxAdmission::new(
        sandbox,
        SandboxAdmissionName::Allocate("Launch capacity".into()),
        8,
        NOW.into(),
    )
    .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI replica set"]
async fn concurrent_launches_reserve_exactly_one_last_home_and_replay_once() {
    for n in [50, 100, 200] {
        let seed = seeded("launch_home_boundary").await;
        let _resources = resources(&seed).await;
        let trigger = published(&seed, "home_boundary").await;
        configure(
            &seed,
            AdmissionCeilings {
                retained_homes: Some(1),
                ..Default::default()
            },
        )
        .await;
        let barrier = Arc::new(tokio::sync::Barrier::new(n));
        let results = futures_util::future::join_all((0..n).map(|index| {
            let store = seed.store.clone();
            let request = request(&trigger, index);
            let barrier = barrier.clone();
            async move {
                barrier.wait().await;
                (request.clone(), store.admit_launch(request).await)
            }
        }))
        .await;
        let mut winner = None;
        let mut refused = 0;
        for (request, result) in results {
            match result {
                Ok(LaunchAdmissionResult::Created(_)) => {
                    assert!(winner.replace(request).is_none());
                }
                Err(LaunchStoreError::GlobalCapacityReached) => refused += 1,
                other => panic!("unexpected Launch capacity result at N={n}: {other:?}"),
            }
        }
        assert_eq!(refused, n - 1);
        let winner = winner.unwrap();
        let reopened =
            MongoTriggerDefinitionStore::open(&seed.client, &seed.database, seed.profiles.clone())
                .await
                .unwrap();
        assert!(matches!(
            reopened.admit_launch(winner).await.unwrap(),
            LaunchAdmissionResult::Existing(_)
        ));
        assert_eq!(pending(&seed).await, 1);
        eprintln!("trigger_retained_homes N={n} limit=1 admitted=1 refused={refused} pending=1 overshoot=0");
        seed.client.database(&seed.database).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI replica set"]
async fn launch_reserve_protects_existing_consumers_and_env_cap_remains_authoritative() {
    let mut seed = seeded("launch_home_reserve").await;
    let store = resources(&seed).await;
    let trigger = published(&seed, "home_reserve").await;
    configure(
        &seed,
        AdmissionCeilings {
            retained_homes: Some(2),
            existing_reserve: ExistingReserve {
                retained_homes: Some(1),
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .await;
    let first = request(&trigger, 0);
    seed.store.admit_launch(first).await.unwrap();
    let public = request(&trigger, 1);
    seed.client
        .database(&seed.database)
        .collection::<Document>("sandbox_admissions")
        .insert_one(doc! {
            "_id": {"scope": "registration", "subject": public.launch.owner.as_str()},
            "document_schema_version": 1_i64,
        })
        .await
        .unwrap();
    assert_eq!(
        seed.store.admit_launch(public).await.unwrap_err(),
        LaunchStoreError::GlobalCapacityReached
    );
    let existing = request(&trigger, 2);
    seed.store.admit_launch(existing).await.unwrap();
    assert_eq!(pending(&seed).await, 2);
    assert_eq!(store.sandboxes.count_documents(doc! {}).await.unwrap(), 0);
    seed.store = seed.store.clone().with_global_sandbox_limit(1);
    assert_eq!(
        seed.store
            .admit_launch(request(&trigger, 3))
            .await
            .unwrap_err(),
        LaunchStoreError::GlobalCapacityReached
    );
    seed.client.database(&seed.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI replica set"]
async fn launch_home_reservation_transfers_after_lowering_and_cancel_releases_once() {
    let seed = seeded("launch_home_transfer").await;
    let store = resources(&seed).await;
    let trigger = published(&seed, "home_transfer").await;
    configure(
        &seed,
        AdmissionCeilings {
            retained_homes: Some(1),
            ..Default::default()
        },
    )
    .await;
    let first = seed
        .store
        .admit_launch(request(&trigger, 0))
        .await
        .unwrap()
        .launch()
        .clone();
    let ordinary = request(&trigger, 1).launch;
    assert_eq!(
        store
            .admit_sandbox(home(&seed, &ordinary, "sbx_ordinary_full", true))
            .await
            .unwrap_err(),
        SandboxStoreError::GlobalCapacityReached
    );
    configure(
        &seed,
        AdmissionCeilings {
            retained_homes: Some(0),
            ..Default::default()
        },
    )
    .await;
    store
        .admit_sandbox(home(&seed, &first, "sbx_launch_transfer", false))
        .await
        .unwrap();
    assert_eq!(pending(&seed).await, 0);
    assert_eq!(
        receipt(&seed, &first).await.get_str("state").unwrap(),
        "consumed"
    );
    configure(
        &seed,
        AdmissionCeilings {
            retained_homes: Some(2),
            ..Default::default()
        },
    )
    .await;
    let mut second = seed
        .store
        .admit_launch(request(&trigger, 2))
        .await
        .unwrap()
        .launch()
        .clone();
    let expected_revision = second.revision;
    let from = second.state;
    second.cancel(LATER.into()).unwrap();
    let commit = LaunchCommit {
        expected_revision,
        launch: second.clone(),
        event_at: LATER.into(),
        events: vec![
            LaunchEventKind::StateChanged {
                from,
                to: LaunchState::Cancelled,
            },
            LaunchEventKind::Cancelled,
        ],
    };
    seed.store.commit_launch(commit.clone()).await.unwrap();
    assert_eq!(pending(&seed).await, 0);
    assert_eq!(
        receipt(&seed, &second).await.get_str("state").unwrap(),
        "released"
    );
    assert!(matches!(
        seed.store.commit_launch(commit).await,
        Err(LaunchStoreError::Conflict { .. })
    ));
    assert_eq!(pending(&seed).await, 0);
    seed.store.admit_launch(request(&trigger, 3)).await.unwrap();
    seed.client.database(&seed.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI replica set"]
async fn resolving_an_existing_home_releases_the_unused_slot_and_preserves_its_marker() {
    let seed = seeded("launch_home_resolved").await;
    let _resources = resources(&seed).await;
    let trigger = published(&seed, "home_resolved").await;
    configure(
        &seed,
        AdmissionCeilings {
            retained_homes: Some(1),
            ..Default::default()
        },
    )
    .await;
    let mut launch = seed
        .store
        .admit_launch(request(&trigger, 0))
        .await
        .unwrap()
        .launch()
        .clone();
    let expected_revision = launch.revision;
    let sandbox_id = layer2_core::SandboxId("sbx_already_owned".into());
    launch.resolution.sandbox = ResolvedResource::Reused(sandbox_id.clone());
    launch.updated_at = LATER.into();
    launch.revision += 1;
    let mut changed = seed
        .store
        .commit_launch(LaunchCommit {
            expected_revision,
            launch,
            event_at: LATER.into(),
            events: vec![LaunchEventKind::SandboxResolved {
                sandbox_id,
                reused: true,
            }],
        })
        .await
        .unwrap();
    assert_eq!(pending(&seed).await, 0);
    assert_eq!(
        receipt(&seed, &changed).await.get_str("state").unwrap(),
        "reused"
    );
    let admissions = seed
        .client
        .database(&seed.database)
        .collection::<Document>("sandbox_admissions");
    let before = admissions
        .find_one(crate::store::launch_home_capacity::fence_key())
        .await
        .unwrap()
        .unwrap();
    let expected_revision = changed.revision;
    changed.updated_at = LATEST.into();
    changed.revision += 1;
    seed.store
        .commit_launch(LaunchCommit {
            expected_revision,
            launch: changed.clone(),
            event_at: LATEST.into(),
            events: vec![],
        })
        .await
        .unwrap();
    let after = admissions
        .find_one(crate::store::launch_home_capacity::fence_key())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        before, after,
        "unchanged reuse must not contend on the capacity fence"
    );
    let row = seed
        .client
        .database(&seed.database)
        .collection::<Document>(LAUNCHES)
        .find_one(doc! {"_id": changed.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert!(row.get_bool("home_capacity_admission").unwrap());
    seed.store.admit_launch(request(&trigger, 1)).await.unwrap();
    assert_eq!(pending(&seed).await, 1);
    seed.client.database(&seed.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI replica set"]
async fn reused_launch_does_not_reserve_a_second_home_and_unreadable_fresh_policy_fails_closed() {
    let seed = seeded("launch_home_reused").await;
    let store = resources(&seed).await;
    let trigger = published(&seed, "home_reused").await;
    configure(
        &seed,
        AdmissionCeilings {
            retained_homes: Some(0),
            ..Default::default()
        },
    )
    .await;
    let mut reused = request(&trigger, 0);
    reused.launch.resolution.sandbox =
        ResolvedResource::Reused(layer2_core::SandboxId("sbx_retained".into()));
    let admitted = seed
        .store
        .admit_launch(reused)
        .await
        .unwrap()
        .launch()
        .clone();
    assert_eq!(pending(&seed).await, 0);
    assert_eq!(
        receipt(&seed, &admitted).await.get_str("state").unwrap(),
        "reused"
    );
    assert_eq!(
        store
            .admit_sandbox(home(&seed, &admitted, "sbx_reused_fallback", false))
            .await
            .unwrap_err(),
        SandboxStoreError::GlobalCapacityReached
    );
    seed.client
        .database(&seed.database)
        .collection::<Document>("platform_settings")
        .update_one(
            doc! {"_id": "platform"},
            doc! {"$set": {"admission_ceilings": null}},
        )
        .await
        .unwrap();
    assert_eq!(
        seed.store
            .admit_launch(request(&trigger, 1))
            .await
            .unwrap_err(),
        LaunchStoreError::GlobalCapacityReached
    );
    seed.client.database(&seed.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI replica set"]
async fn never_configured_launches_add_no_capacity_commands_or_receipts() {
    use mongodb::event::{command::CommandEvent, EventHandler};
    use mongodb::options::ClientOptions;
    let seed = seeded("launch_home_unset").await;
    let _resources = resources(&seed).await;
    let trigger = published(&seed, "home_unset").await;
    let trace = Arc::new(std::sync::Mutex::new(Vec::new()));
    let observed = trace.clone();
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            observed
                .lock()
                .unwrap()
                .push((event.command_name, event.command));
        }
    }));
    let client = mongodb::Client::with_options(options).unwrap();
    let store = MongoTriggerDefinitionStore::open(&client, &seed.database, seed.profiles.clone())
        .await
        .unwrap();
    trace.lock().unwrap().clear();
    store.admit_launch(request(&trigger, 0)).await.unwrap();
    let missing = trace.lock().unwrap().clone();
    seed.client.database(&seed.database).collection::<Document>("platform_settings").update_one(doc! {"_id": "platform"}, doc! {"$set": {"admission_ceilings": mongodb::bson::serialize_to_bson(&layer2_core::admission::AdmissionCeilingsState::default()).unwrap()}}).await.unwrap();
    trace.lock().unwrap().clear();
    let admitted = store
        .admit_launch(request(&trigger, 1))
        .await
        .unwrap()
        .launch()
        .clone();
    let unset = trace.lock().unwrap().clone();
    assert_eq!(
        missing.iter().map(|(name, _)| name).collect::<Vec<_>>(),
        unset.iter().map(|(name, _)| name).collect::<Vec<_>>()
    );
    assert!(
        !unset.iter().any(|(_, command)| command.get_str("find").ok()
            == Some("sandbox_admissions")
            || command.get_str("aggregate").ok() == Some("sandboxes")
            || command.get_str("update").ok() == Some("sandbox_admissions")
            || command.get_str("insert").ok() == Some("sandbox_admissions"))
    );
    let row = seed
        .client
        .database(&seed.database)
        .collection::<Document>(LAUNCHES)
        .find_one(doc! {"_id": admitted.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert!(!row.contains_key("home_capacity_admission"));
    assert_eq!(
        seed.client
            .database(&seed.database)
            .collection::<Document>("sandbox_admissions")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    eprintln!("trigger_retained_homes unset commands={} extra_commands=0 capacity_counts=0 capacity_fences=0 receipt_writes=0", unset.len());
    seed.client.database(&seed.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI replica set"]
async fn unactivated_deployment_cap_keeps_legacy_worker_admission() {
    let mut seed = seeded("launch_home_activation").await;
    seed.store = seed.store.clone().with_global_sandbox_limit(1);
    let store = resources(&seed).await.with_global_sandbox_limit(1);
    let trigger = published(&seed, "home_activation").await;
    let first = seed
        .store
        .admit_launch(request(&trigger, 0))
        .await
        .unwrap()
        .launch()
        .clone();
    let admissions = seed
        .client
        .database(&seed.database)
        .collection::<Document>("sandbox_admissions");
    let launches = seed
        .client
        .database(&seed.database)
        .collection::<Document>(LAUNCHES);
    let row = launches
        .find_one(doc! {"_id":first.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert!(!row.contains_key("home_capacity_admission"));
    assert_eq!(admissions.count_documents(doc! {}).await.unwrap(), 0);
    let second = seed
        .store
        .admit_launch(request(&trigger, 1))
        .await
        .unwrap()
        .launch()
        .clone();
    // Before activation only the worker consumes #1672's deployment slot.
    store
        .admit_sandbox(home(&seed, &first, "sbx_legacy_first", false))
        .await
        .unwrap();
    assert_eq!(
        store
            .admit_sandbox(home(&seed, &second, "sbx_legacy_second", false))
            .await
            .unwrap_err(),
        SandboxStoreError::GlobalCapacityReached
    );
    assert_eq!(
        store
            .admit_sandbox(home(
                &seed,
                &request(&trigger, 2).launch,
                "sbx_legacy_manual",
                true
            ))
            .await
            .unwrap_err(),
        SandboxStoreError::GlobalCapacityReached
    );
    let fence = admissions
        .find_one(crate::store::launch_home_capacity::fence_key())
        .await
        .unwrap()
        .unwrap();
    assert!(!fence.contains_key("pending_launch_homes"));
    assert_eq!(
        admissions.count_documents(doc! {}).await.unwrap(),
        2,
        "only the inherited owner and global fences may exist"
    );
    assert_eq!(
        launches
            .count_documents(doc! {"home_capacity_admission":{"$exists":true}})
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        admissions
            .count_documents(doc! {"_id.scope":"launch_home_capacity"})
            .await
            .unwrap(),
        0
    );
    assert!(admissions
        .find_one(doc! {"_id":{"scope":"registration_capacity"}})
        .await
        .unwrap()
        .is_none());

    crate::store::MongoRegistrationCapacityStore::open(&seed.client, &seed.database, true)
        .await
        .unwrap();
    // The already-open Launch store observes activation without restarting.
    seed.store = seed.store.clone().with_global_sandbox_limit(2);
    let activated = seed
        .store
        .admit_launch(request(&trigger, 3))
        .await
        .unwrap()
        .launch()
        .clone();
    assert_eq!(pending(&seed).await, 1);
    assert_eq!(
        receipt(&seed, &activated).await.get_str("state").unwrap(),
        "reserved"
    );
    assert!(launches
        .find_one(doc! {"_id":activated.id.as_str()})
        .await
        .unwrap()
        .unwrap()
        .get_bool("home_capacity_admission")
        .unwrap());
    assert_eq!(
        seed.store
            .admit_launch(request(&trigger, 4))
            .await
            .unwrap_err(),
        LaunchStoreError::GlobalCapacityReached
    );
    seed.client.database(&seed.database).drop().await.unwrap();
}
