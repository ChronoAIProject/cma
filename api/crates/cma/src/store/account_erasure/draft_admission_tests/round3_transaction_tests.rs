//! Real transaction failures retain the preexisting mutation error contracts.
use super::*;
use crate::store::{sandbox_mongo::MongoProfileStore, settings::MongoPlatformSettingsStore};
use layer2_core::{PlatformSettings, PlatformSettingsStore, StoreError};
use mongodb::{options::ClientOptions, Client};

#[derive(Clone, Copy, Debug)]
pub(super) enum Failure {
    WriteConflict,
    UnknownCommit,
}

async fn operation_client() -> (Client, Client, String) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI")
        .expect("CHRONO_TEST_MONGODB_FAILPOINT_URI must name a failpoint-enabled replica set");
    let app_name = format!("cma-draft-contract-{}", uuid::Uuid::new_v4().simple());
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app_name.clone());
    (admin, Client::with_options(options).unwrap(), app_name)
}

pub(super) async fn inject<T>(
    admin: &Client,
    app_name: &str,
    command: &str,
    failure: Failure,
    operation: impl std::future::Future<Output = T>,
) -> T {
    let (mode, data) = match failure {
        Failure::WriteConflict => (
            mongodb::bson::Bson::Document(doc! {"times":1_i32}),
            doc! {
                "failCommands":[command], "appName":app_name,
                "errorCode":112_i32, "errorLabels":["TransientTransactionError"],
            },
        ),
        Failure::UnknownCommit => (
            mongodb::bson::Bson::String("alwaysOn".into()),
            doc! {
                "failCommands":["commitTransaction"], "appName":app_name,
                "errorCode":1_i32, "errorLabels":["UnknownTransactionCommitResult"],
            },
        ),
    };
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand", "mode":mode, "data":data})
        .await
        .unwrap();
    let result = operation.await;
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand", "mode":"off"})
        .await
        .expect("disable the isolated failpoint before checking the result");
    result
}

async fn draft_failure(failure: Failure, workspace_case: bool) {
    let seed = seeded("round3_draft_transaction_errors").await;
    let (admin, client, app_name) = operation_client().await;
    let store = MongoProfileStore::open(&client, &seed.database)
        .await
        .unwrap();
    if workspace_case {
        let mut workspace = seed
            .profiles
            .workspace_profile(&seed.target.workspace_profile.id)
            .await
            .unwrap()
            .unwrap();
        workspace.id = WorkspaceProfileId("wkp_round3_transaction".into());
        seed.profiles
            .create_workspace_profile(workspace.clone())
            .await
            .unwrap();
        let mut changed = workspace.clone();
        changed.name = "Updated workspace".into();
        changed.draft_version = 2;
        let result = inject(
            &admin,
            &app_name,
            "update",
            failure,
            store.save_workspace_draft(1, changed.clone()),
        )
        .await;
        assert_eq!(result, Err(ProfileStoreError::Conflict), "{failure:?}");
        assert_eq!(
            store.workspace_profile(&workspace.id).await.unwrap(),
            Some(workspace)
        );
        store
            .save_workspace_draft(1, changed.clone())
            .await
            .unwrap();
        assert_eq!(
            store.workspace_profile(&changed.id).await.unwrap(),
            Some(changed)
        );
    } else {
        let mut sandbox = seed
            .profiles
            .sandbox_profile(&seed.target.sandbox_profile.id)
            .await
            .unwrap()
            .unwrap();
        sandbox.id = SandboxProfileId("sbp_round3_transaction".into());
        seed.profiles
            .create_sandbox_profile(sandbox.clone())
            .await
            .unwrap();
        let mut changed = sandbox.clone();
        changed.name = "Updated sandbox".into();
        changed.draft_version = 2;
        let result = inject(
            &admin,
            &app_name,
            "update",
            failure,
            store.save_sandbox_draft(1, changed.clone()),
        )
        .await;
        assert_eq!(result, Err(ProfileStoreError::Conflict), "{failure:?}");
        assert_eq!(
            store.sandbox_profile(&sandbox.id).await.unwrap(),
            Some(sandbox)
        );
        store.save_sandbox_draft(1, changed.clone()).await.unwrap();
        assert_eq!(
            store.sandbox_profile(&changed.id).await.unwrap(),
            Some(changed)
        );
    }
    seed.client.database(&seed.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn workspace_draft_write_conflict_keeps_the_conflict_contract() {
    draft_failure(Failure::WriteConflict, true).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn workspace_draft_unknown_commit_keeps_the_conflict_contract() {
    draft_failure(Failure::UnknownCommit, true).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn sandbox_draft_write_conflict_keeps_the_conflict_contract() {
    draft_failure(Failure::WriteConflict, false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn sandbox_draft_unknown_commit_keeps_the_conflict_contract() {
    draft_failure(Failure::UnknownCommit, false).await;
}

async fn settings_failure(failure: Failure) {
    let seed = seeded("round3_settings_transaction_errors").await;
    let (admin, client, app_name) = operation_client().await;
    let store = MongoPlatformSettingsStore::open(&client, &seed.database)
        .await
        .unwrap();
    let expected = store
        .replace(0, &PlatformSettings::default())
        .await
        .unwrap();
    let mut changed = expected.settings.clone();
    changed.inactivity.pause_after_seconds += 60;
    let result = inject(
        &admin,
        &app_name,
        "findAndModify",
        failure,
        store.replace(expected.revision, &changed),
    )
    .await;
    assert_eq!(
        result,
        Err(StoreError::RevisionConflict {
            expected: expected.revision,
            actual: expected.revision,
        }),
        "{failure:?}"
    );
    assert_eq!(store.read().await.unwrap(), expected);
    assert_eq!(
        store
            .replace(expected.revision, &changed)
            .await
            .unwrap()
            .settings,
        changed
    );
    seed.client.database(&seed.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn platform_settings_write_conflict_keeps_the_revision_conflict_contract() {
    settings_failure(Failure::WriteConflict).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn platform_settings_unknown_commit_keeps_the_revision_conflict_contract() {
    settings_failure(Failure::UnknownCommit).await;
}

async fn fork_failure(failure: Failure) {
    let seed = seeded("round3_fork_transaction_errors").await;
    let (admin, client, app_name) = operation_client().await;
    let store = MongoProfileStore::open(&client, &seed.database)
        .await
        .unwrap();
    let owner = SubjectId::new("survivor");
    let locator = MarketplaceLocator::Agent(seed.target.agent_profile.id.clone());
    let result = inject(
        &admin,
        &app_name,
        "insert",
        failure,
        store.fork_profile(&owner, &locator, 1, NOW.into()),
    )
    .await;
    assert!(matches!(result, Err(MarketplaceError::Unavailable(_))));
    store
        .fork_profile(&owner, &locator, 1, NOW.into())
        .await
        .unwrap();
    assert_eq!(
        seed.client
            .database(&seed.database)
            .collection::<Document>("agent_profiles")
            .count_documents(doc! {"owner":owner.as_str()})
            .await
            .unwrap(),
        1
    );
    seed.client.database(&seed.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn marketplace_fork_write_conflict_keeps_the_existing_unavailable_contract() {
    fork_failure(Failure::WriteConflict).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn marketplace_fork_unknown_commit_keeps_the_existing_unavailable_contract() {
    fork_failure(Failure::UnknownCommit).await;
}
