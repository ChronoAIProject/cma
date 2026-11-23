//! Real store failures preserve create and settings mutation error contracts.
use super::{
    round3_transaction_tests::{inject, Failure},
    *,
};
use crate::store::{
    sandbox_mongo::MongoProfileStore, schema::DOCUMENT_SCHEMA_VERSION,
    settings::MongoPlatformSettingsStore, trigger_definitions::integration_tests::Seeded,
};
use layer2_core::{
    profiles::{SandboxProfile, WorkspaceProfile},
    ActivityAction, ActivityQuery, ActivityService, ActivityStore, CoreError, PlatformSettings,
    PlatformSettingsService, PlatformSettingsStore, StoreError, StoredPlatformSettings,
};
use mongodb::{
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
    Client,
};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};

#[derive(Clone)]
enum Candidate {
    Workspace(Box<WorkspaceProfile>),
    Sandbox(Box<SandboxProfile>),
}

impl Candidate {
    async fn load(seed: &Seeded, workspace: bool) -> Self {
        if workspace {
            let mut profile = seed
                .profiles
                .workspace_profile(&seed.target.workspace_profile.id)
                .await
                .unwrap()
                .unwrap();
            profile.id = WorkspaceProfileId("wkp_round4_create".into());
            profile.owner = SubjectId::new("survivor");
            Self::Workspace(Box::new(profile))
        } else {
            let mut profile = seed
                .profiles
                .sandbox_profile(&seed.target.sandbox_profile.id)
                .await
                .unwrap()
                .unwrap();
            profile.id = SandboxProfileId("sbp_round4_create".into());
            profile.owner = SubjectId::new("survivor");
            Self::Sandbox(Box::new(profile))
        }
    }

    async fn create(&self, store: &MongoProfileStore) -> Result<(), ProfileStoreError> {
        match self {
            Self::Workspace(profile) => {
                store
                    .create_workspace_profile(profile.as_ref().clone())
                    .await
            }
            Self::Sandbox(profile) => store.create_sandbox_profile(profile.as_ref().clone()).await,
        }
    }

    async fn assert_stored(&self, store: &MongoProfileStore) {
        match self {
            Self::Workspace(profile) => assert_eq!(
                store.workspace_profile(&profile.id).await.unwrap().as_ref(),
                Some(profile.as_ref())
            ),
            Self::Sandbox(profile) => assert_eq!(
                store.sandbox_profile(&profile.id).await.unwrap().as_ref(),
                Some(profile.as_ref())
            ),
        }
    }

    fn identity(&self) -> (&'static str, &str) {
        match self {
            Self::Workspace(profile) => ("workspace_profiles", profile.id.as_str()),
            Self::Sandbox(profile) => ("sandbox_profiles", profile.id.as_str()),
        }
    }
}

async fn create_contract(workspace: bool) {
    let seed = seeded("round4_create_contract").await;
    let db = seed.client.database(&seed.database);
    let candidate = Candidate::load(&seed, workspace).await;
    let (children, child_id) = if workspace {
        ("agent_profiles", seed.target.agent_profile.id.as_str())
    } else {
        (
            "workspace_profiles",
            seed.target.workspace_profile.id.as_str(),
        )
    };
    fence(&db, children, child_id, true).await;
    assert_eq!(
        candidate.create(&seed.profiles).await,
        Err(ProfileStoreError::ReferenceUnavailable)
    );
    let (collection, id) = candidate.identity();
    assert_eq!(
        db.collection::<Document>(collection)
            .count_documents(doc! {"_id":id})
            .await
            .unwrap(),
        0
    );
    fence(&db, children, child_id, false).await;
    candidate.create(&seed.profiles).await.unwrap();
    candidate.assert_stored(&seed.profiles).await;
    assert_eq!(
        candidate.create(&seed.profiles).await,
        Err(ProfileStoreError::Duplicate)
    );
    candidate.assert_stored(&seed.profiles).await;
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn workspace_create_preserves_success_duplicate_and_fenced_child_contracts() {
    create_contract(true).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn sandbox_create_preserves_success_duplicate_and_fenced_child_contracts() {
    create_contract(false).await;
}

async fn operation_client() -> (Client, Client, String, Arc<AtomicUsize>) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI")
        .expect("CHRONO_TEST_MONGODB_FAILPOINT_URI must name a failpoint-enabled replica set");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let app_name = format!("cma-create-contract-{}", uuid::Uuid::new_v4().simple());
    let inserts = Arc::new(AtomicUsize::new(0));
    let observed = inserts.clone();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app_name.clone());
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if matches!(event, CommandEvent::Started(event) if event.command_name == "insert") {
            observed.fetch_add(1, Ordering::SeqCst);
        }
    }));
    (
        admin,
        Client::with_options(options).unwrap(),
        app_name,
        inserts,
    )
}

async fn create_failure(failure: Failure, workspace: bool) {
    let seed = seeded("round4_create_transaction_errors").await;
    let (admin, client, app_name, inserts) = operation_client().await;
    let store = MongoProfileStore::open(&client, &seed.database)
        .await
        .unwrap();
    let candidate = Candidate::load(&seed, workspace).await;
    inserts.store(0, Ordering::SeqCst);
    let result = inject(
        &admin,
        &app_name,
        "insert",
        failure,
        candidate.create(&store),
    )
    .await;
    assert_eq!(
        inserts.load(Ordering::SeqCst),
        1,
        "create must not replay its mutation"
    );
    assert!(
        matches!(result, Err(ProfileStoreError::Unavailable(_))),
        "{failure:?}: {result:?}"
    );
    let (collection, id) = candidate.identity();
    assert_eq!(
        seed.client
            .database(&seed.database)
            .collection::<Document>(collection)
            .count_documents(doc! {"_id":id})
            .await
            .unwrap(),
        0
    );
    candidate.create(&store).await.unwrap();
    candidate.assert_stored(&store).await;
    assert_eq!(inserts.load(Ordering::SeqCst), 2);
    seed.client.database(&seed.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn workspace_create_write_conflict_remains_unavailable_without_mutation_replay() {
    create_failure(Failure::WriteConflict, true).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn workspace_create_unknown_commit_remains_unavailable_without_mutation_replay() {
    create_failure(Failure::UnknownCommit, true).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn sandbox_create_write_conflict_remains_unavailable_without_mutation_replay() {
    create_failure(Failure::WriteConflict, false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn sandbox_create_unknown_commit_remains_unavailable_without_mutation_replay() {
    create_failure(Failure::UnknownCommit, false).await;
}

struct CorruptBeforeReplace {
    store: Arc<MongoPlatformSettingsStore>,
    database: Database,
    armed: AtomicBool,
}

#[async_trait::async_trait]
impl PlatformSettingsStore for CorruptBeforeReplace {
    async fn read(&self) -> Result<StoredPlatformSettings, StoreError> {
        self.store.read().await
    }

    async fn replace(
        &self,
        expected: u64,
        settings: &PlatformSettings,
    ) -> Result<StoredPlatformSettings, StoreError> {
        if self.armed.swap(false, Ordering::SeqCst) {
            assert_eq!(
                self.database
                    .collection::<Document>("platform_settings")
                    .update_one(
                        doc! {"_id":"platform"},
                        doc! {"$set":{"document_schema_version": DOCUMENT_SCHEMA_VERSION + 1}}
                    )
                    .await
                    .unwrap()
                    .matched_count,
                1
            );
        }
        self.store.replace(expected, settings).await
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn persisted_settings_corruption_during_mutation_remains_unavailable_and_repairs_normally() {
    let seed = seeded("round4_settings_mutation_corruption").await;
    let db = seed.client.database(&seed.database);
    let (store, activity, _, caller) =
        super::round3_settings_service_tests::service(&seed, &seed.client).await;
    let previous = store
        .replace(0, &PlatformSettings::default())
        .await
        .unwrap();
    let wrapper = Arc::new(CorruptBeforeReplace {
        store: store.clone(),
        database: db.clone(),
        armed: AtomicBool::new(true),
    });
    let service = PlatformSettingsService::new(wrapper, ActivityService::new(activity.clone()));
    let mut changed = previous.settings.clone();
    changed.inactivity.pause_after_seconds += 60;
    let result = service
        .configure_platform(&caller, previous.revision, &changed)
        .await;
    assert!(
        matches!(result, Err(CoreError::RuntimeUnavailable(_))),
        "{result:?}"
    );
    let persisted = db
        .collection::<Document>("platform_settings")
        .find_one(doc! {"_id":"platform"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        persisted.get_i64("revision").unwrap(),
        previous.revision as i64
    );
    assert_eq!(
        mongodb::bson::deserialize_from_bson::<PlatformSettings>(persisted["settings"].clone())
            .unwrap(),
        previous.settings
    );
    assert!(activity
        .list(&ActivityQuery::default())
        .await
        .unwrap()
        .items
        .is_empty());
    db.collection::<Document>("platform_settings")
        .update_one(
            doc! {"_id":"platform"},
            doc! {"$set":{"document_schema_version":DOCUMENT_SCHEMA_VERSION}},
        )
        .await
        .unwrap();
    assert_eq!(store.read().await.unwrap(), previous);
    let configured = service
        .configure_platform(&caller, previous.revision, &changed)
        .await
        .unwrap();
    assert_eq!(configured.revision, previous.revision + 1);
    assert_eq!(
        configured.settings.inactivity.pause_after_seconds,
        changed.inactivity.pause_after_seconds
    );
    let records = activity
        .list(&ActivityQuery::default())
        .await
        .unwrap()
        .items;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].action, ActivityAction::Configure);
    db.drop().await.unwrap();
}
