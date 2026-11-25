use layer2_core::ids::{SandboxId, SandboxProfileId, ServiceId, SubjectId};
use layer2_core::profiles::{
    content_digest, AgentProvider, ModelAccess, SandboxProfileContent, SandboxProfileSnapshot,
};
use layer2_core::sandbox::store::{
    SandboxAdmission, SandboxAdmissionName, SandboxAdmissionResult, SandboxAdmissionStore,
    SandboxEventStore, SandboxStore, SandboxStoreError,
};
use layer2_core::sandbox::{
    Sandbox, SandboxCredentialState, SandboxLimits, SandboxOrigin, SandboxSignInState,
    SandboxStatus, SandboxWorkState, SandboxWorkStep,
};
use mongodb::bson::doc;
use mongodb::Client;

use super::sandbox_mongo::{is_duplicate_key, MongoSandboxStore};
use super::sandboxes::admitted_sandbox_document;

const NOW: &str = "2026-09-05T00:00:00Z";
const LATER: &str = "2026-09-05T00:01:00Z";

#[path = "sandbox_admission/canary_tests.rs"]
mod canary;
#[path = "sandbox_admission/runtime_image_origin_mongo_tests.rs"]
mod runtime_image_origin;

mod visitor_limit;

#[path = "sandbox_admission_always_on_service_mongo_tests.rs"]
mod always_on_service;

#[path = "warm_sandboxes/mongo_integration_tests.rs"]
mod warmup;

#[path = "sandbox_admission_wake_mongo_tests.rs"]
mod admission_wake;

async fn database(prefix: &str) -> (Client, String) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(uri).await.unwrap();
    let name = format!("cma_821_{prefix}_{}", uuid::Uuid::new_v4().simple());
    (client, name)
}

pub(crate) fn sandbox(id: &str, launch_id: &str) -> Sandbox {
    let content = SandboxProfileContent {
        runtime_image: None,
        cpu_millis: 1_000,
        memory_mib: 2_048,
        nyxid_services: vec![ServiceId::new("profile-service")],
        agent_provider: AgentProvider::Codex,
        model_access: ModelAccess::OwnCredentials,
        max_workspaces: 4,
        max_agents_per_workspace: 4,
        workspace_profiles: vec![],
        software_install_commands: vec![],
        environment_variables: vec![],
    };
    Sandbox {
        id: SandboxId(id.into()),
        owner: SubjectId("owner".into()),
        name: "trigger-research".into(),
        origin: SandboxOrigin::Trigger {
            launch_id: launch_id.into(),
        },
        profile: SandboxProfileSnapshot {
            id: SandboxProfileId("sbp_trigger".into()),
            revision: 1,
            digest: content_digest(&content).unwrap(),
            always_on: None,
            content,
        },
        status: SandboxStatus::Provisioning,
        failure: None,
        runtime: None,
        credential: SandboxCredentialState {
            source: Default::default(),
            service_grants: Vec::new(),
            service_scope: None,
            reaches: vec![ServiceId::new("profile-service")],
            key_generation: 0,
            delivered: false,
            current: None,
            pending: None,
            revoke_debt: vec![],
            sign_in: SandboxSignInState::Required,
        },
        work: SandboxWorkState::new(SandboxWorkStep::ProvisionRuntime, NOW.into()),
        limits: SandboxLimits {
            max_workspaces: 4,
            max_agents_per_workspace: 4,
        },
        lifecycle: Default::default(),
        last_activity_at: NOW.into(),
        created_at: NOW.into(),
        updated_at: NOW.into(),
        retired_at: None,
        purge_after: None,
        revision: 1,
    }
}

fn admission(id: &str, launch_id: &str, limit: u16) -> SandboxAdmission {
    SandboxAdmission::new(
        sandbox(id, launch_id),
        SandboxAdmissionName::Allocate("trigger-research".into()),
        limit,
        NOW.into(),
    )
    .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn trigger_replay_survives_lifecycle_changes_and_precedes_zero_quota() {
    let (client, database) = database("replay").await;
    let store = MongoSandboxStore::open(&client, &database).await.unwrap();
    let first = admission("sbx_first", "launch-replay", 1);
    let created = match store.admit_sandbox(first.clone()).await.unwrap() {
        SandboxAdmissionResult::Created(sandbox) => sandbox,
        SandboxAdmissionResult::Existing(_) => panic!("fresh trigger admission replayed"),
    };
    let document = store
        .sandboxes
        .find_one(doc! { "_id": created.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(document.get_str("origin_type").unwrap(), "trigger");
    assert_eq!(
        document.get_str("origin_launch_id").unwrap(),
        "launch-replay"
    );
    assert_eq!(
        document.get_str("admission_fingerprint").unwrap(),
        first.admission_fingerprint
    );
    assert_eq!(
        document.get_document("admission_name").unwrap(),
        &doc! { "mode": "allocate", "value": "trigger-research" }
    );
    let replay = store
        .sandbox_admission_replay(
            &SubjectId::new("owner"),
            &SandboxOrigin::Trigger {
                launch_id: "launch-replay".into(),
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replay.sandbox, created);
    assert_eq!(replay.name, first.name);
    assert_eq!(replay.admission_fingerprint, first.admission_fingerprint);
    assert!(store
        .sandbox_admission_replay(
            &SubjectId::new("other"),
            &SandboxOrigin::Trigger {
                launch_id: "launch-replay".into()
            }
        )
        .await
        .unwrap()
        .is_none());
    assert!(store
        .sandbox_admission_replay(
            &SubjectId::new("owner"),
            &SandboxOrigin::Trigger {
                launch_id: "launch-missing".into()
            }
        )
        .await
        .unwrap()
        .is_none());

    let mut advanced = created;
    advanced.status = SandboxStatus::Active;
    advanced.credential.sign_in = SandboxSignInState::Complete;
    advanced.updated_at = LATER.into();
    advanced.revision = 2;
    store.save_sandbox(1, advanced.clone()).await.unwrap();
    let document = store
        .sandboxes
        .find_one(doc! { "_id": advanced.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        document.get_document("admission_name").unwrap(),
        &doc! { "mode": "allocate", "value": "trigger-research" }
    );
    assert_eq!(
        document.get_str("admission_fingerprint").unwrap(),
        first.admission_fingerprint
    );
    assert_eq!(
        store
            .sandbox_admission_replay(
                &SubjectId::new("owner"),
                &SandboxOrigin::Trigger {
                    launch_id: "launch-replay".into()
                }
            )
            .await
            .unwrap()
            .unwrap()
            .sandbox,
        advanced
    );
    assert_eq!(
        store
            .admit_sandbox(admission("sbx_retry", "launch-replay", 0))
            .await
            .unwrap(),
        SandboxAdmissionResult::Existing(advanced)
    );

    let mut changed = sandbox("sbx_changed", "launch-replay");
    changed
        .credential
        .reaches
        .push(ServiceId::new("expanded-security-reach"));
    let changed = SandboxAdmission::new(
        changed,
        SandboxAdmissionName::Allocate("trigger-research".into()),
        0,
        NOW.into(),
    )
    .unwrap();
    assert_eq!(
        store.admit_sandbox(changed).await.unwrap_err(),
        SandboxStoreError::SandboxOriginConflict
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn replay_discovery_rejects_hidden_query_field_drift_and_duplicate_identity() {
    let (client, database) = database("corrupt_replay_query").await;
    let store = MongoSandboxStore::open(&client, &database).await.unwrap();
    let first = admission("sbx_corrupt_origin", "launch-corrupt-origin", 3);
    let created = match store.admit_sandbox(first).await.unwrap() {
        SandboxAdmissionResult::Created(sandbox) => sandbox,
        SandboxAdmissionResult::Existing(_) => panic!("fresh trigger admission replayed"),
    };
    store
        .sandboxes
        .update_one(
            doc! { "_id": created.id.as_str() },
            doc! { "$set": { "origin_launch_id": "launch-hidden-origin" } },
        )
        .await
        .unwrap();

    assert!(matches!(
        store
            .sandbox_admission_replay(
                &SubjectId::new("owner"),
                &SandboxOrigin::Trigger {
                    launch_id: "launch-corrupt-origin".into()
                }
            )
            .await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    assert!(matches!(
        store
            .admit_sandbox(admission("sbx_corrupt_retry", "launch-corrupt-origin", 3))
            .await,
        Err(SandboxStoreError::Corrupt(_))
    ));

    let mut duplicate = sandbox("sbx_corrupt_duplicate", "launch-corrupt-origin");
    duplicate.name = "different-live-name".into();
    let duplicate = SandboxAdmission::new(
        duplicate,
        SandboxAdmissionName::Exact("different-live-name".into()),
        3,
        NOW.into(),
    )
    .unwrap();
    let duplicate_document = admitted_sandbox_document(
        &duplicate.sandbox,
        1,
        &duplicate.name,
        &duplicate.admission_fingerprint,
    )
    .unwrap();
    let error = store
        .sandboxes
        .insert_one(duplicate_document)
        .await
        .unwrap_err();
    assert!(is_duplicate_key(&error));
    assert_eq!(
        store
            .sandboxes
            .count_documents(doc! {
                "record.owner": "owner",
                "record.origin.type": "trigger",
                "record.origin.launch_id": "launch-corrupt-origin",
            })
            .await
            .unwrap(),
        1
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn concurrent_trigger_admissions_converge_on_one_origin_row_and_event() {
    let (client, database) = database("concurrent_replay").await;
    let store = MongoSandboxStore::open(&client, &database).await.unwrap();
    let results = futures_util::future::join_all((0..12).map(|index| {
        let store = store.clone();
        async move {
            store
                .admit_sandbox(admission(
                    &format!("sbx_concurrent_{index}"),
                    "launch-concurrent",
                    1,
                ))
                .await
        }
    }))
    .await;
    let mut created = 0;
    let mut ids = Vec::new();
    for result in results {
        match result.unwrap() {
            SandboxAdmissionResult::Created(sandbox) => {
                created += 1;
                ids.push(sandbox.id);
            }
            SandboxAdmissionResult::Existing(sandbox) => ids.push(sandbox.id),
        }
    }
    assert_eq!(created, 1);
    assert!(ids.iter().all(|id| id == &ids[0]));
    assert_eq!(
        store
            .sandboxes
            .count_documents(doc! { "owner": "owner", "origin_launch_id": "launch-concurrent" })
            .await
            .unwrap(),
        1
    );
    assert_eq!(store.sandbox_events(&ids[0], 0, 10).await.unwrap().len(), 1);
    let indexes = store.sandboxes.list_index_names().await.unwrap();
    assert!(indexes.contains(&"sandboxes_owner_trigger_launch".to_string()));
    client.database(&database).drop().await.unwrap();
}

#[path = "sandbox_global_admission_mongo_tests.rs"]
mod global_admission;

#[path = "sandbox_admission/ceiling_mongo_tests.rs"]
mod ceilings;

#[path = "sandbox_admission/pending_home_mongo_tests.rs"]
mod pending_home_ceilings;

#[path = "sandbox_owner_admission_mongo_tests.rs"]
mod owner_replicas;

#[path = "sandbox_admission/public_owner_mongo_tests.rs"]
mod public_owner;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn exact_admission_replay_precedes_deletion_but_new_admission_is_refused() {
    use layer2_core::account_deletion::AccountDeletionStore;
    let (client, database) = database("deletion_replay").await;
    let store = MongoSandboxStore::open(&client, &database).await.unwrap();
    let created = match store
        .admit_sandbox(admission("sbx_original", "launch-original", 1))
        .await
        .unwrap()
    {
        SandboxAdmissionResult::Created(record) => record,
        SandboxAdmissionResult::Existing(_) => panic!("fresh admission replayed"),
    };
    let deletions =
        super::account_deletions::MongoAccountDeletionStore::open(&client.database(&database))
            .await
            .unwrap();
    deletions
        .request(&SubjectId::new("owner"), "delete", NOW)
        .await
        .unwrap();
    assert_eq!(
        store
            .admit_sandbox(admission("sbx_replay", "launch-original", 0))
            .await
            .unwrap(),
        SandboxAdmissionResult::Existing(created.clone())
    );
    assert_eq!(
        store
            .admit_sandbox(admission("sbx_new", "launch-new", 1))
            .await,
        Err(SandboxStoreError::AccountDeletionInProgress)
    );
    assert_eq!(
        store
            .sandboxes
            .count_documents(doc! {"owner":"owner"})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        store
            .sandbox_events(&created.id, 0, 10)
            .await
            .unwrap()
            .len(),
        1
    );
    client.database(&database).drop().await.unwrap();
}
