//! Zero-liability commitments read the platform admission state inside their
//! transaction and never write the settings fence or the period row.
use super::budget_tests::{
    configure_metered, configure_with_platform, observe, open_platform, period_row,
};
use super::*;
use layer2_core::budget::{BudgetError, Closure, ClosureReason};

async fn enter(store: &MongoSandboxStore, reason: ClosureReason) {
    let (start, _) = open_platform().period_bounds().unwrap();
    let closure = Closure {
        reason,
        since: start,
        settings_revision: 1,
        committed_at_entry: 0,
    };
    store
        .model_budget
        .update_one(
            doc! {"_id": crate::store::budget::period_key(start)},
            doc! {"$push": {"record.closures": mongodb::bson::serialize_to_bson(&closure).unwrap()},
            "$inc": {"record.revision": 1_i64}},
        )
        .await
        .unwrap();
}

async fn fence(store: &MongoSandboxStore) -> Option<i64> {
    store
        .budget_settings
        .find_one(doc! {"_id": "platform"})
        .await
        .unwrap()
        .and_then(|document| document.get_i64("model_budget_admission_fence").ok())
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn create_sandbox_is_refused_under_spend_stop_and_admitted_when_open() {
    let (client, name) = database("platform_create_sandbox").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let owner = provisioning_sandbox("sbx_platform_owner").owner;
    configure_metered(&store, &owner).await;
    let before = (
        fence(&store).await,
        period_row(&store, &open_platform()).await,
    );
    assert!(matches!(
        store.admit_sandbox(admission("sbx_platform_open", 8)).await,
        Ok(SandboxAdmissionResult::Created(_))
    ));
    assert_eq!(
        (
            fence(&store).await,
            period_row(&store, &open_platform()).await
        ),
        before,
        "a zero-liability admission writes neither hot document"
    );
    enter(&store, ClosureReason::SpendStop).await;
    assert_eq!(
        store
            .admit_sandbox(admission("sbx_platform_stopped", 8))
            .await
            .unwrap_err(),
        SandboxStoreError::Budget(BudgetError::PlatformExhausted)
    );
    assert_eq!(store.sandboxes.count_documents(doc! {}).await.unwrap(), 1);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn create_sandbox_is_refused_once_committed_equals_disposable_and_enters_the_stop() {
    let (client, name) = database("platform_create_at_disposable").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let owner = provisioning_sandbox("sbx_platform_owner").owner;
    let platform = layer2_core::budget::PlatformBudgetPolicy {
        period_cap: 1_000,
        ..open_platform()
    };
    configure_with_platform(&store, &owner, platform.clone(), 1).await;
    // Observed half a day ago at one unit per day: the projection adds
    // exactly one unit for the rest of this test.
    let now = layer2_core::seconds_from_rfc3339(&layer2_core::now_rfc3339()).unwrap();
    let observed_through = now - 43_200;
    observe(&store, &platform, 998, observed_through).await;
    assert!(matches!(
        store
            .admit_sandbox(admission("sbx_platform_below", 8))
            .await,
        Ok(SandboxAdmissionResult::Created(_))
    ));
    assert!(period_row(&store, &platform)
        .await
        .unwrap()
        .closures
        .is_empty());
    observe(&store, &platform, 999, observed_through).await;
    assert_eq!(
        store
            .admit_sandbox(admission("sbx_platform_at_disposable", 8))
            .await
            .unwrap_err(),
        SandboxStoreError::Budget(BudgetError::PlatformExhausted)
    );
    let row = period_row(&store, &platform).await.unwrap();
    assert_eq!(
        row.closures
            .iter()
            .map(|closure| closure.reason)
            .collect::<Vec<_>>(),
        vec![ClosureReason::SpendStop, ClosureReason::SpendWarning]
    );
    assert_eq!(row.closures[0].committed_at_entry, 1_000);
    assert_eq!(store.sandboxes.count_documents(doc! {}).await.unwrap(), 1);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn workspace_and_agent_admission_are_refused_under_spend_stop() {
    let (client, name) = database("platform_children").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let parent = active_sandbox("sbx_platform_children");
    store.create_sandbox(parent.clone()).await.unwrap();
    let ready = ready_workspace(&parent, "wks_platform_ready", "project");
    store.create_workspace(ready.clone()).await.unwrap();
    configure_metered(&store, &parent.owner).await;
    assert!(matches!(
        store
            .admit_agent(agent_admission(&ready, "platform-open", "platform_open"))
            .await,
        Ok(AgentAdmissionResult::Created(_))
    ));
    enter(&store, ClosureReason::SpendStop).await;
    assert_eq!(
        store
            .admit_workspace(workspace_admission(workspace(
                &parent,
                "wks_platform_new",
                "new-project"
            )))
            .await,
        Err(SandboxStoreError::Budget(BudgetError::PlatformExhausted))
    );
    assert_eq!(
        store
            .admit_agent(agent_admission(
                &ready,
                "platform-stopped",
                "platform_stopped"
            ))
            .await,
        Err(SandboxStoreError::Budget(BudgetError::PlatformExhausted))
    );
    assert_eq!(store.workspaces.count_documents(doc! {}).await.unwrap(), 1);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn metering_unavailable_refuses_commitments_with_budget_unavailable() {
    let (client, name) = database("platform_unmetered").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let owner = provisioning_sandbox("sbx_platform_owner").owner;
    // Cloud is configured but never observed: the source is stale, not zero.
    configure_with_platform(&store, &owner, open_platform(), 1).await;
    assert_eq!(
        store
            .admit_sandbox(admission("sbx_platform_unmetered", 8))
            .await
            .unwrap_err(),
        SandboxStoreError::Budget(BudgetError::Unavailable)
    );
    let row = period_row(&store, &open_platform()).await.unwrap();
    assert_eq!(
        row.closures
            .iter()
            .map(|closure| closure.reason)
            .collect::<Vec<_>>(),
        vec![ClosureReason::MeteringUnavailable]
    );
    assert_eq!((row.reserved, row.spent), (0, 0));
    assert_eq!(store.sandboxes.count_documents(doc! {}).await.unwrap(), 0);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn no_platform_policy_means_no_period_row_and_no_refusal() {
    let (client, name) = database("platform_absent").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert!(store
        .admit_sandbox(admission("sbx_platform_no_settings", 8))
        .await
        .is_ok());
    store
        .budget_settings
        .replace_one(
            doc! {"_id": "platform"},
            doc! {"_id": "platform", "revision": 1_i64,
            "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "settings": mongodb::bson::serialize_to_bson(
                &layer2_core::PlatformSettings::default()).unwrap()},
        )
        .upsert(true)
        .await
        .unwrap();
    assert!(store
        .admit_sandbox(admission("sbx_platform_disabled", 8))
        .await
        .is_ok());
    assert_eq!(
        store
            .model_budget
            .count_documents(doc! {"_id": {"$regex": "^platform:"}})
            .await
            .unwrap(),
        0
    );
    client.database(&name).drop().await.unwrap();
}
