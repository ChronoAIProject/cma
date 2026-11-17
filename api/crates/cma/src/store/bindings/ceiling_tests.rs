//! Bound homes consume fleet capacity as existing-consumer continuations.
use super::admission_tests::sandbox;
use super::mongo_tests::{binding, fixture, NOW};
use super::*;
use crate::store::admission_ceilings::MongoAdmissionCeilingsStore;
use layer2_core::admission::{
    AdmissionCause, AdmissionCeilings, AdmissionCeilingsStore, AdmissionControlStore,
    ExistingReserve,
};
use layer2_core::sandbox::binding::{BindingAdmissionFence, BindingState, BindingWork};
use layer2_core::sandbox::store::{
    SandboxAdmission, SandboxAdmissionName, SandboxAdmissionResult, SandboxAdmissionStore,
    SandboxStore,
};
use layer2_core::SubjectId;

async fn admitted_binding(store: &MongoSandboxStore) -> SandboxAdmission {
    let mut value = binding();
    store.save_binding(None, value.clone()).await.unwrap();
    value.state = BindingState::Provisioning;
    value.work = BindingWork::ProvisionSandbox;
    value.advance_revision(NOW.into()).unwrap();
    store.save_binding(Some(1), value.clone()).await.unwrap();
    let parent = sandbox(&value);
    let mut request = SandboxAdmission::new(
        parent.clone(),
        SandboxAdmissionName::Exact(parent.name),
        3,
        NOW.into(),
    )
    .unwrap();
    request.binding = Some(BindingAdmissionFence::environment(&value));
    assert_eq!(request.admission_cause(), AdmissionCause::Continuation);
    request
}

async fn configure(client: &mongodb::Client, database: &str, limit: u32, reserve: Option<u32>) {
    crate::store::MongoRegistrationCapacityStore::open(client, database, true)
        .await
        .unwrap();
    let store = MongoAdmissionCeilingsStore::open(client, database, 0)
        .await
        .unwrap();
    let revision = store.read().await.unwrap().revision;
    store
        .replace(
            revision,
            &SubjectId::new("operator"),
            "configure bound home capacity",
            AdmissionCeilings {
                retained_homes: Some(limit),
                existing_reserve: ExistingReserve {
                    retained_homes: reserve,
                    ..Default::default()
                },
                ..Default::default()
            },
            100 + revision,
        )
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn bound_home_growth_obeys_capacity_and_pending_slots_with_existing_reserve() {
    let (client, database, store) = fixture().await;
    let request = admitted_binding(&store).await;
    configure(&client, &database, 0, None).await;
    assert_eq!(
        store.admit_sandbox(request.clone()).await.unwrap_err(),
        SandboxStoreError::GlobalCapacityReached
    );
    assert_eq!(store.sandboxes.count_documents(doc! {}).await.unwrap(), 0);

    configure(&client, &database, 1, Some(1)).await;
    // A binding context remains an existing consumer even when its human
    // owner also has a post-activation public registration receipt.
    store
        .sandbox_admissions
        .insert_one(doc! {
            "_id": {"scope": "registration", "subject": request.sandbox.owner.as_str()},
            "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        })
        .await
        .unwrap();
    store
        .sandbox_admissions
        .update_one(
            doc! {"_id": {"scope": "global_sandbox_capacity"}},
            doc! {"$set": {"pending_launch_homes": 1_i64}, "$inc": {"fence": 1_i64},
            "$setOnInsert": {"document_schema_version": DOCUMENT_SCHEMA_VERSION}},
        )
        .upsert(true)
        .await
        .unwrap();
    assert_eq!(
        store.admit_sandbox(request.clone()).await.unwrap_err(),
        SandboxStoreError::GlobalCapacityReached
    );
    store
        .sandbox_admissions
        .update_one(
            doc! {"_id": {"scope": "global_sandbox_capacity"}},
            doc! {"$set": {"pending_launch_homes": 0_i64}, "$inc": {"fence": 1_i64}},
        )
        .await
        .unwrap();
    crate::store::admission_control::MongoAdmissionControlStore::open(&client, &database)
        .await
        .unwrap()
        .change(
            0,
            &SubjectId::new("operator"),
            "contain incident",
            true,
            100,
        )
        .await
        .unwrap();
    let created = match store.admit_sandbox(request).await.unwrap() {
        SandboxAdmissionResult::Created(sandbox) => sandbox,
        other => panic!("unexpected bound home result: {other:?}"),
    };
    configure(&client, &database, 0, None).await;
    // Binding recovery reads the exact retained home before asking for a new
    // insertion. Lowering policy leaves that existing resource readable.
    assert_eq!(store.get_sandbox(&created.id).await.unwrap(), Some(created));
    assert_eq!(store.sandboxes.count_documents(doc! {}).await.unwrap(), 1);
    assert_eq!(
        store.sandbox_events.count_documents(doc! {}).await.unwrap(),
        1
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn unreadable_ceilings_do_not_strand_a_bound_home_continuation() {
    let (client, database, store) = fixture().await;
    let request = admitted_binding(&store).await;
    store
        .budget_settings
        .update_one(
            doc! {"_id": "platform"},
            doc! {"$set": {"admission_ceilings": null}},
        )
        .await
        .unwrap();
    assert!(matches!(
        store.admit_sandbox(request).await.unwrap(),
        SandboxAdmissionResult::Created(_)
    ));
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn unreadable_ceilings_refuse_a_fresh_binding_home_before_the_binding_is_saved() {
    use layer2_core::sandbox::binding::ExistingBindingAgent;
    use layer2_core::{AgentId, WorktreeId};
    let (client, database, store) = fixture().await;
    store
        .budget_settings
        .update_one(
            doc! {"_id": "platform"},
            doc! {"$set": {"admission_ceilings": null}},
        )
        .await
        .unwrap();
    let mut fresh = binding();
    assert_eq!(
        store.save_binding(None, fresh.clone()).await,
        Err(SandboxStoreError::GlobalCapacityReached)
    );
    assert_eq!(
        store
            .bindings
            .records
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    let control =
        crate::store::admission_control::MongoAdmissionControlStore::open(&client, &database)
            .await
            .unwrap();
    control
        .change(
            0,
            &SubjectId::new("operator"),
            "contain incident",
            true,
            100,
        )
        .await
        .unwrap();
    assert_eq!(
        store.save_binding(None, fresh.clone()).await,
        Err(SandboxStoreError::AdmissionPaused)
    );
    control
        .change(
            1,
            &SubjectId::new("operator"),
            "resume admission",
            false,
            101,
        )
        .await
        .unwrap();
    // A binding that borrows an existing Agent creates no home and keeps its
    // original admission behavior under unreadable retained-home policy.
    fresh.existing_agent = Some(ExistingBindingAgent {
        agent_id: AgentId("agt_borrowed".into()),
        worktree_id: WorktreeId("wt_borrowed".into()),
    });
    fresh.level = 1;
    fresh.state = BindingState::Ready;
    fresh.work = BindingWork::Idle;
    store.save_binding(None, fresh).await.unwrap();
    client.database(&database).drop().await.unwrap();
}
