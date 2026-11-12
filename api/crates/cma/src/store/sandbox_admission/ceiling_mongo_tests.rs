//! Operator ceilings share the retained-home transaction fence.
use super::*;
use crate::store::sandbox_mongo::DOCUMENT_SCHEMA_VERSION;
use layer2_core::admission::{AdmissionCeilings, ExistingReserve};

pub(super) fn fresh(index: usize) -> SandboxAdmission {
    let mut row = sandbox(&format!("sbx_ceiling_{index}"), "unused");
    row.owner = SubjectId::new(format!("ceiling-owner-{index}"));
    row.origin = SandboxOrigin::Manual;
    row.name = "capacity".into();
    SandboxAdmission::new(
        row,
        SandboxAdmissionName::Allocate("capacity".into()),
        8,
        NOW.into(),
    )
    .unwrap()
}

pub(super) async fn configure(store: &MongoSandboxStore, ceilings: AdmissionCeilings) {
    crate::store::MongoRegistrationCapacityStore::open(
        &store.client,
        &store.sandboxes.namespace().db,
        true,
    )
    .await
    .unwrap();
    let row = store
        .budget_settings
        .find_one(doc! {"_id": "platform"})
        .await
        .unwrap()
        .unwrap();
    let state = crate::store::admission_ceilings::from_document(&row).unwrap();
    let next = state
        .changed(
            state.revision,
            &SubjectId::new("operator"),
            "configure retained-home capacity",
            ceilings,
            100,
        )
        .unwrap()
        .unwrap();
    store
        .budget_settings
        .update_one(
            doc! {"_id": "platform"},
            doc! {"$set": {"admission_ceilings": mongodb::bson::serialize_to_bson(&next).unwrap()}},
        )
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn fifty_one_hundred_and_two_hundred_owners_cannot_overshoot_the_last_home() {
    for n in [50, 100, 200] {
        let (client, name) = database("home_boundary").await;
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        store.admit_sandbox(fresh(n)).await.unwrap();
        configure(
            &store,
            AdmissionCeilings {
                retained_homes: Some(2),
                ..Default::default()
            },
        )
        .await;
        let other = MongoSandboxStore::open(&client, &name).await.unwrap();
        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(n));
        let results = futures_util::future::join_all((0..n).map(|index| {
            let store = if index % 2 == 0 {
                store.clone()
            } else {
                other.clone()
            };
            let barrier = barrier.clone();
            async move {
                barrier.wait().await;
                store.admit_sandbox(fresh(index)).await
            }
        }))
        .await;
        let mut admitted = 0;
        let mut refused = 0;
        for result in results {
            match result {
                Ok(SandboxAdmissionResult::Created(_)) => admitted += 1,
                Err(SandboxStoreError::GlobalCapacityReached) => refused += 1,
                other => panic!("unexpected home outcome at N={n}: {other:?}"),
            }
        }
        assert_eq!((admitted, refused), (1, n - 1));
        eprintln!("retained_homes N={n} existing=1 limit=2 admitted={admitted} refused={refused} overshoot=0");
        assert_eq!(store.sandboxes.count_documents(doc! {}).await.unwrap(), 2);
        assert_eq!(
            store.sandbox_events.count_documents(doc! {}).await.unwrap(),
            2
        );
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn public_home_refusal_preserves_existing_reserve_and_lowering_keeps_rows() {
    let (client, name) = database("home_reserve").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    configure(
        &store,
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
    store.admit_sandbox(fresh(0)).await.unwrap();
    store
        .sandbox_admissions
        .insert_one(doc! {
            "_id": {"scope": "registration", "subject": fresh(1).sandbox.owner.as_str()},
            "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "registered_at": 100_i64,
        })
        .await
        .unwrap();
    assert_eq!(
        store.admit_sandbox(fresh(1)).await.unwrap_err(),
        SandboxStoreError::GlobalCapacityReached
    );
    store.admit_sandbox(fresh(2)).await.unwrap();
    configure(
        &store,
        AdmissionCeilings {
            retained_homes: Some(1),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(
        store.admit_sandbox(fresh(3)).await.unwrap_err(),
        SandboxStoreError::GlobalCapacityReached
    );
    assert_eq!(store.sandboxes.count_documents(doc! {}).await.unwrap(), 2);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn deployment_home_cap_wins_and_admitted_launch_keeps_its_original_rule() {
    let (client, name) = database("home_hard_cap").await;
    let store = MongoSandboxStore::open(&client, &name)
        .await
        .unwrap()
        .with_global_sandbox_limit(1);
    configure(
        &store,
        AdmissionCeilings {
            retained_homes: Some(9),
            ..Default::default()
        },
    )
    .await;
    store.admit_sandbox(fresh(0)).await.unwrap();
    assert_eq!(
        store.admit_sandbox(fresh(1)).await.unwrap_err(),
        SandboxStoreError::GlobalCapacityReached
    );
    configure(
        &store,
        AdmissionCeilings {
            retained_homes: Some(0),
            ..Default::default()
        },
    )
    .await;
    let unlimited_deployment = store.clone().with_global_sandbox_limit(0);
    assert_eq!(
        unlimited_deployment
            .admit_sandbox(fresh(2))
            .await
            .unwrap_err(),
        SandboxStoreError::GlobalCapacityReached
    );
    let mut continuation = fresh(3);
    continuation.sandbox.origin = SandboxOrigin::Trigger {
        launch_id: "admitted-launch".into(),
    };
    let continuation =
        SandboxAdmission::new(continuation.sandbox, continuation.name, 8, NOW.into()).unwrap();
    unlimited_deployment
        .admit_sandbox(continuation.clone())
        .await
        .unwrap();
    assert!(matches!(
        unlimited_deployment
            .admit_sandbox(continuation)
            .await
            .unwrap(),
        SandboxAdmissionResult::Existing(_)
    ));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn unreadable_ceilings_close_only_fresh_home_admission() {
    let (client, name) = database("home_unreadable").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    crate::store::MongoRegistrationCapacityStore::open(&client, &name, true)
        .await
        .unwrap();
    store
        .budget_settings
        .update_one(
            doc! {"_id": "platform"},
            doc! {"$set": {"admission_ceilings": "unreadable"}},
        )
        .await
        .unwrap();
    assert_eq!(
        store.admit_sandbox(fresh(0)).await.unwrap_err(),
        SandboxStoreError::GlobalCapacityReached
    );
    let mut continued = fresh(1).sandbox;
    continued.origin = SandboxOrigin::Trigger {
        launch_id: "accepted-before-corruption".into(),
    };
    store
        .admit_sandbox(
            SandboxAdmission::new(
                continued,
                SandboxAdmissionName::Allocate("capacity".into()),
                8,
                NOW.into(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn unset_home_ceiling_adds_zero_mongo_commands_and_uses_no_capacity_fence() {
    use mongodb::event::{command::CommandEvent, EventHandler};
    use mongodb::options::ClientOptions;
    let trace = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
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
    let client = Client::with_options(options).unwrap();
    let name = format!("cma_home_zero_commands_{}", uuid::Uuid::new_v4().simple());
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    trace.lock().unwrap().clear();
    store.admit_sandbox(fresh(0)).await.unwrap();
    let missing = trace.lock().unwrap().clone();
    store.budget_settings.update_one(doc! {"_id": "platform"}, doc! {"$set": {"admission_ceilings": mongodb::bson::serialize_to_bson(&layer2_core::admission::AdmissionCeilingsState::default()).unwrap()}}).await.unwrap();
    trace.lock().unwrap().clear();
    store.admit_sandbox(fresh(1)).await.unwrap();
    let unset = trace.lock().unwrap().clone();
    assert_eq!(
        missing.iter().map(|(name, _)| name).collect::<Vec<_>>(),
        unset.iter().map(|(name, _)| name).collect::<Vec<_>>()
    );
    assert_eq!(
        store
            .sandbox_admissions
            .count_documents(doc! {"_id": {"scope": "global_sandbox_capacity"}})
            .await
            .unwrap(),
        0
    );
    assert!(!unset
        .iter()
        .any(|(_, command)| command.get_str("find").ok() == Some("sandbox_admissions")));
    assert_eq!(
        unset
            .iter()
            .filter(|(_, command)| command.get_str("find").ok() == Some("platform_settings"))
            .count(),
        1
    );
    assert_eq!(
        unset
            .iter()
            .filter(|(_, command)| command.get_str("aggregate").ok() == Some("sandboxes"))
            .count(),
        1,
        "only the inherited per-owner count runs"
    );
    eprintln!("retained_homes unset commands={} extra_commands=0 settings_reads=1 owner_counts=1 capacity_fences=0", unset.len());
    client.database(&name).drop().await.unwrap();
}
