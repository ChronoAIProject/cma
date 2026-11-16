//! Service policy changes survive fresh Mongo composition and Agent ancestry reads.

use super::*;
use futures_util::FutureExt;
use layer2_core::profiles::{
    InMemoryProfileStore, SandboxProfileRef, WorkspaceLocation, WorkspaceProfileContent,
    WorkspaceProfileSnapshot,
};
use layer2_core::sandbox::store::{PageRequest, WorkspaceStore, WorktreeStore};
use layer2_core::sandbox::{
    Agent, AgentReadError, AgentReadService, CreateSandbox, FixedSandboxCredentialPrerequisite,
    OperatorError, RuntimeBinding, SandboxOperatorService, SandboxPolicy, SandboxResourceReadError,
    SandboxResourceReader, SandboxResourceReading, SandboxService, SandboxServiceError,
    StoredSandboxRetirement, Workspace, WorkspaceOrigin, WorkspaceSource, WorkspaceStatus,
    WorkspaceWorkState, WorkspaceWorkStep, Worktree, WorktreeStatus,
};
use layer2_core::{
    ActivityAction, ActivityQuery, ActivityService, ActivityStore, Authorization, Caller,
    Capability, DetailKey, WorkspaceProfileId,
};
use std::{future::IntoFuture, panic::AssertUnwindSafe, sync::Arc, time::Duration};

use crate::store::sandbox_maintenance::mongo_integration_tests::{insert_agent, ready_agent};
use crate::store::MongoActivityStore;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn lifecycle_service_policy_changes_survive_reopen_and_agent_reads() {
    service_case(SandboxOrigin::Lifecycle {
        operation_id: "always-on-service-operation".into(),
    })
    .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn trigger_service_policy_changes_survive_reopen_and_agent_reads() {
    service_case(SandboxOrigin::Trigger {
        launch_id: "always-on-service-launch".into(),
    })
    .await;
}

async fn service_case(origin: SandboxOrigin) {
    let (client, name) = database("policy_service").await;
    let database = client.database(&name);
    assert!(database.list_collection_names().await.unwrap().is_empty());
    let result = tokio::time::timeout(
        Duration::from_secs(120),
        AssertUnwindSafe(exercise(&client, &name, origin)).catch_unwind(),
    )
    .await;
    let cleanup =
        tokio::time::timeout(Duration::from_secs(10), database.drop().into_future()).await;
    assert!(
        matches!(cleanup, Ok(Ok(()))),
        "owned database cleanup failed"
    );
    eprintln!("always_on_service_database_cleanup=PASS");
    match result {
        Ok(Ok(())) => {}
        Ok(Err(panic)) => std::panic::resume_unwind(panic),
        Err(_) => panic!("Always-on service Mongo regression timed out"),
    }
}

fn owner(subject: &str) -> Caller {
    Caller::new(
        SubjectId(subject.into()),
        Authorization::new([
            Capability::UseOwnSandboxes,
            Capability::CreateSandbox,
            Capability::LaunchTriggers,
        ]),
    )
}

fn administrator() -> Caller {
    Caller::new(
        SubjectId("policy-operator".into()),
        Authorization::new([Capability::ControlAnySandbox, Capability::ReadAllSandboxes]),
    )
}

fn owner_service(store: Arc<MongoSandboxStore>) -> Arc<SandboxService> {
    SandboxService::new(
        store,
        Arc::new(InMemoryProfileStore::default()),
        SandboxPolicy::default(),
        Arc::new(FixedSandboxCredentialPrerequisite::Ready),
    )
}

async fn operator_service(
    client: &Client,
    database: &str,
    store: Arc<MongoSandboxStore>,
) -> SandboxOperatorService {
    let activity = MongoActivityStore::open(client, database, Duration::from_secs(3600))
        .await
        .unwrap();
    SandboxOperatorService::new(
        store.clone(),
        Arc::new(InMemoryProfileStore::default()),
        Arc::new(StoredSandboxRetirement::new(store.clone())),
        ActivityService::new(Arc::new(activity)),
    )
    .with_maintenance(store)
}

async fn exercise(client: &Client, database: &str, origin: SandboxOrigin) {
    let store = Arc::new(MongoSandboxStore::open(client, database).await.unwrap());
    let mut original = sandbox("sbx_policy_service", "unused");
    original.origin = origin;
    let admission = SandboxAdmission::new(
        original.clone(),
        SandboxAdmissionName::Exact(original.name.clone()),
        1,
        NOW.into(),
    )
    .unwrap();
    assert_eq!(
        store.admit_sandbox(admission.clone()).await.unwrap(),
        SandboxAdmissionResult::Created(original.clone())
    );
    let mut current = original;
    current.status = SandboxStatus::Active;
    current.credential.sign_in = SandboxSignInState::Complete;
    current.runtime = Some(RuntimeBinding {
        boot_id: None,
        runtime_id: "policy-runtime".into(),
        runtime_version: "policy-fixture-image".into(),
        provision_generation: Some(1),
        bound_at: NOW.into(),
    });
    current.lifecycle.runtime_generation_high_water = 1;
    current.work = SandboxWorkState::new(SandboxWorkStep::WaitForCredentialRotation, NOW.into());
    current.revision += 1;
    store.save_sandbox(1, current.clone()).await.unwrap();
    let agent = insert_hierarchy(&store, &current).await;
    let owner = owner(current.owner.as_str());
    let operator = administrator();
    let owner_service = owner_service(store.clone());
    let admin_service = operator_service(client, database, store.clone()).await;
    assert_reopened(database, &current, &admission, &agent).await;

    for policy in [true, false, true] {
        let previous_revision = current.revision;
        assert_eq!(
            owner_service
                .set_always_on(&owner, &current.id, previous_revision, true)
                .await,
            Err(SandboxServiceError::AlwaysOnApprovalRequired)
        );
        assert_eq!(
            admin_service
                .set_always_on(&owner, &current.id, previous_revision, policy)
                .await,
            Err(OperatorError::Forbidden)
        );
        current = admin_service
            .set_always_on(&operator, &current.id, previous_revision, policy)
            .await
            .unwrap();
        assert_eq!(current.profile.always_on, Some(policy));
        assert_eq!(current.revision, previous_revision + 1);
        assert!(matches!(
            admin_service
                .set_always_on(&operator, &current.id, previous_revision, !policy)
                .await,
            Err(OperatorError::InvalidState(_))
        ));
        assert_reopened(database, &current, &admission, &agent).await;
    }

    let revision = current.revision;
    current = owner_service
        .set_always_on(&owner, &current.id, revision, false)
        .await
        .unwrap();
    assert_eq!(current.profile.always_on, Some(false));
    assert_eq!(current.revision, revision + 1);
    assert_eq!(
        owner_service
            .set_always_on(&owner, &current.id, revision, false)
            .await,
        Err(SandboxServiceError::InvalidState)
    );
    assert_eq!(
        owner_service
            .set_always_on(&owner, &current.id, current.revision, false)
            .await
            .unwrap(),
        current,
        "relinquishing a disabled policy must not advance its revision"
    );
    drop(admin_service);
    drop(owner_service);
    drop(store);
    // Only the retained Mongo documents remain; every service/store is rebuilt.
    assert_reopened(database, &current, &admission, &agent).await;

    let activity = MongoActivityStore::open(client, database, Duration::from_secs(3600))
        .await
        .unwrap();
    let records = activity
        .list(&ActivityQuery {
            actor: Some(operator.subject.clone()),
            action: Some(ActivityAction::Configure),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(
        records.items.len(),
        3,
        "only accepted admin writes are audited"
    );
    for record in &records.items {
        assert_eq!(record.detail.get(DetailKey::Setting), Some("always_on"));
    }
}

async fn assert_reopened(
    database: &str,
    expected: &Sandbox,
    admission: &SandboxAdmission,
    agent: &Agent,
) {
    let client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let store = Arc::new(MongoSandboxStore::open(&client, database).await.unwrap());
    let service = owner_service(store.clone());
    let owner = owner(expected.owner.as_str());
    let stranger = super::always_on_service::owner("unrelated-owner");
    assert_eq!(service.read(&owner, &expected.id).await.unwrap(), *expected);
    assert_eq!(
        service.read(&stranger, &expected.id).await,
        Err(SandboxServiceError::NotFound)
    );
    assert_eq!(
        service
            .set_always_on(&stranger, &expected.id, expected.revision, false)
            .await,
        Err(SandboxServiceError::NotFound)
    );
    let request = CreateSandbox {
        profile: SandboxProfileRef {
            id: admission.sandbox.profile.id.clone(),
            revision: admission.sandbox.profile.revision,
        },
        name: Some(admission.sandbox.name.clone()),
        origin: admission.sandbox.origin.clone(),
        confirmed_digest: None,
    };
    let replay = match &request.origin {
        SandboxOrigin::Lifecycle { .. } => service.create_for_lifecycle(&owner, request).await,
        SandboxOrigin::Trigger { .. } => {
            service
                .create_for_launch(&owner, Capability::LaunchTriggers, request)
                .await
        }
        SandboxOrigin::Manual => unreachable!("only replay-bound origins are exercised"),
    }
    .unwrap();
    assert_eq!(replay, *expected);
    let retained = store
        .sandbox_admission_replay(&owner.subject, &expected.origin)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.name, admission.name);
    assert_eq!(
        retained.admission_fingerprint,
        admission.admission_fingerprint
    );
    let inventory = service
        .list_own(&owner, &PageRequest::default())
        .await
        .unwrap();
    assert_eq!(inventory.items, vec![expected.clone()]);
    let admin = operator_service(&client, database, store.clone()).await;
    let detail = admin
        .read_sandbox(&administrator(), &expected.id)
        .await
        .unwrap();
    assert_eq!(detail.sandbox, *expected);
    assert_eq!(detail.agents.len(), 1);
    assert_eq!(detail.agents[0].id, agent.id);

    let reads = AgentReadService::new(store.clone(), Arc::new(NoRuntimeResources));
    assert_eq!(reads.read(&owner, &agent.id).await.unwrap(), *agent);
    let lineage = reads.lineage(&owner, &agent.id).await.unwrap();
    assert_eq!(lineage.agent, *agent);
    assert_eq!(lineage.sandbox, *expected);
    assert_eq!(lineage.workspace.id, agent.workspace_id);
    assert_eq!(lineage.worktree.id, agent.worktree_id);
    assert_eq!(
        reads.read(&stranger, &agent.id).await,
        Err(AgentReadError::NotFound)
    );
    assert_eq!(store.sandboxes.count_documents(doc! {}).await.unwrap(), 1);
    assert_eq!(
        store
            .sandbox_events(&expected.id, 0, 10)
            .await
            .unwrap()
            .len(),
        1
    );
}

async fn insert_hierarchy(store: &MongoSandboxStore, parent: &Sandbox) -> Agent {
    let agent = ready_agent(parent, "agt_policy_service");
    let content = WorkspaceProfileContent {
        location: WorkspaceLocation::Directory {
            name: "project".into(),
        },
        name: "project".into(),
        setup_commands: vec![],
        agent_profiles: vec![],
    };
    store
        .create_workspace(Workspace {
            id: agent.workspace_id.clone(),
            sandbox_id: parent.id.clone(),
            owner: parent.owner.clone(),
            origin: WorkspaceOrigin::Manual,
            profile: WorkspaceProfileSnapshot {
                id: WorkspaceProfileId("wkp_policy_service".into()),
                revision: 1,
                digest: content_digest(&content).unwrap(),
                content,
            },
            name: "project".into(),
            path: "/home/cma/project".into(),
            source: WorkspaceSource::Empty,
            resolved_commit: None,
            status: WorkspaceStatus::Ready,
            work: WorkspaceWorkState::new(WorkspaceWorkStep::None, NOW.into()),
            next_worktree_number: 2,
            created_at: NOW.into(),
            ready_at: Some(NOW.into()),
            failed: None,
            removed_at: None,
            revision: 1,
        })
        .await
        .unwrap();
    store
        .create_worktree(Worktree {
            id: agent.worktree_id.clone(),
            workspace_id: agent.workspace_id.clone(),
            sandbox_id: parent.id.clone(),
            owner: parent.owner.clone(),
            name: "wt-1".into(),
            path: "/home/cma/project/.worktrees/wt-1".into(),
            branch: "cma/wt-1".into(),
            base_commit: "c".repeat(40),
            status: WorktreeStatus::Ready,
            agent_id: Some(agent.id.clone()),
            revision: 1,
        })
        .await
        .unwrap();
    // Retained conversation fixture; no runtime is launched by these persistence tests.
    insert_agent(store, &agent, NOW).await;
    agent
}

struct NoRuntimeResources;

#[async_trait::async_trait]
impl SandboxResourceReader for NoRuntimeResources {
    async fn sandbox_resources(
        &self,
        _sandbox: &Sandbox,
    ) -> Result<SandboxResourceReading, SandboxResourceReadError> {
        panic!("policy replay and Agent ancestry reads must not call a runtime")
    }
}
