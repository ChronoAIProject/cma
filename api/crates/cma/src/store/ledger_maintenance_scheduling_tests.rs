//! Actual automatic pause/rotation ports, including their abort-only preview.
use super::*;
use crate::store::ledger_maintenance_publication::test_support as maintenance;
use crate::store::ledger_maintenance_test_support as support;
use crate::store::sandbox_mongo::DOCUMENT_SCHEMA_VERSION;

#[path = "ledger_maintenance_scheduling_race_tests.rs"]
mod races;

async fn retained_fixture(store: &MongoSandboxStore) -> Sandbox {
    let (mut parent, agent) = fixture(store, "sbx_ledger_scheduling").await;
    let revision = parent.revision;
    parent.runtime.as_mut().unwrap().boot_id = Some("b".repeat(32));
    support::retain_home(&mut parent);
    parent.revision += 1;
    store.save_sandbox(revision, parent.clone()).await.unwrap();
    support::insert_hierarchy(store, &parent, &agent).await;
    // Exercise a real policy-fence write before the later parent publication fence.
    store
        .client
        .database(&store.sandboxes.namespace().db)
        .collection::<mongodb::bson::Document>("sandbox_retained_availability")
        .insert_one(doc! {
            "_id":parent.id.as_str(), "document_schema_version":DOCUMENT_SCHEMA_VERSION,
            "owner":parent.owner.as_str(), "sandbox_created_at":&parent.created_at,
            "profile_id":parent.profile.id.as_str(), "always_keep_alive":false,
            "inactivity_fence":0_i64,
        })
        .await
        .unwrap();
    parent
}

async fn schedule_kind(
    store: &MongoSandboxStore,
    parent: &Sandbox,
    rotate: bool,
) -> Result<SandboxMaintenanceOutcome, SandboxStoreError> {
    if rotate {
        store.schedule_image_rotation(rotation(&parent.id)).await
    } else {
        store.schedule_inactivity_pause(pause(&parent.id)).await
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and scheduling hooks"]
async fn scheduling_succeeds_unheld_and_both_holds_refuse_after_reopen() {
    for rotate in [false, true] {
        for hold in [0, 1, 2] {
            let (client, name, store) = database().await;
            let parent = retained_fixture(&store).await;
            let fence = maintenance::fence_value(&store, &parent).await;
            if hold != 0 {
                maintenance::hold(&store, &parent, hold == 1).await;
            }
            let before = support::all_rows(&store).await;
            let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
            let result = schedule_kind(&reopened, &parent, rotate).await;
            if hold == 0 {
                assert_eq!(result.unwrap(), SandboxMaintenanceOutcome::Scheduled);
                let next = store.get_sandbox(&parent.id).await.unwrap().unwrap();
                assert_eq!(
                    next.status,
                    if rotate {
                        SandboxStatus::Rotating
                    } else {
                        SandboxStatus::Pausing
                    }
                );
                assert_eq!(next.revision, parent.revision + 1);
                assert_eq!(maintenance::fence_value(&store, &parent).await, fence + 1);
                let committed = support::all_rows(&store).await;
                assert_ne!(committed, before);
                assert_eq!(
                    schedule_kind(&reopened, &parent, rotate).await.unwrap(),
                    SandboxMaintenanceOutcome::AlreadyScheduled
                );
                assert_eq!(support::all_rows(&store).await, committed);
            } else {
                assert_eq!(result, Err(SandboxStoreError::Conflict));
                assert_eq!(support::all_rows(&store).await, before);
                assert_eq!(
                    reopened.preview_inactivity_pause(pause(&parent.id)).await,
                    Err(SandboxStoreError::Conflict)
                );
                assert_eq!(support::all_rows(&store).await, before);
            }
            client.database(&name).drop().await.unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and scheduling hooks"]
async fn preview_and_not_eligible_outcomes_roll_back_policy_and_parent_fences() {
    let (client, name, store) = database().await;
    let parent = retained_fixture(&store).await;
    let before = support::all_rows(&store).await;
    assert_eq!(
        store
            .preview_inactivity_pause(pause(&parent.id))
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::Scheduled
    );
    assert_eq!(support::all_rows(&store).await, before);
    let mut early = pause(&parent.id);
    early.requested_at = IDLE_AT.into();
    assert_eq!(
        store.schedule_inactivity_pause(early).await.unwrap(),
        SandboxMaintenanceOutcome::NotEligible
    );
    assert_eq!(support::all_rows(&store).await, before);
    assert_eq!(
        schedule_kind(&store, &parent, false).await.unwrap(),
        SandboxMaintenanceOutcome::Scheduled
    );
    let row = store
        .client
        .database(&name)
        .collection::<mongodb::bson::Document>("sandbox_retained_availability")
        .find_one(doc! {"_id":parent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.get_i64("inactivity_fence").unwrap(), 1);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and scheduling hooks"]
async fn missing_hold_audit_refuses_before_policy_or_child_mutation() {
    for v1 in [false, true] {
        let (client, name, store) = database().await;
        let parent = retained_fixture(&store).await;
        maintenance::hold(&store, &parent, v1).await;
        let audit = if v1 {
            crate::store::ledger_maintenance_transaction::audit_collection(&store)
        } else {
            crate::store::ledger_maintenance_pre_exclusion_transaction::audit_collection(&store)
        };
        audit.delete_many(doc! {}).await.unwrap();
        let before = support::all_rows(&store).await;
        for rotate in [false, true] {
            assert!(matches!(
                schedule_kind(&store, &parent, rotate).await,
                Err(SandboxStoreError::Corrupt(_))
            ));
            assert_eq!(support::all_rows(&store).await, before);
        }
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set with failpoints and scheduling hooks"]
async fn failed_scheduling_commit_rolls_back_every_fence_then_retries() {
    for rotate in [false, true] {
        let (client, name, store) = database().await;
        let parent = retained_fixture(&store).await;
        let before = support::all_rows(&store).await;
        client.database("admin").run_command(doc! {"configureFailPoint":"failCommand","mode":{"times":1},"data":{"failCommands":["commitTransaction"],"errorCode":13}}).await.unwrap();
        let result = schedule_kind(&store, &parent, rotate).await;
        client
            .database("admin")
            .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
            .await
            .unwrap();
        assert!(matches!(result, Err(SandboxStoreError::Unavailable(_))));
        assert_eq!(support::all_rows(&store).await, before);
        support::observe_transaction_cleanup(&store).await;
        assert_eq!(support::all_rows(&store).await, before);
        assert_eq!(
            schedule_kind(&store, &parent, rotate).await.unwrap(),
            SandboxMaintenanceOutcome::Scheduled
        );
        client.database(&name).drop().await.unwrap();
    }
}
