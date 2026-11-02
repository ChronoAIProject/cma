use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use layer2_core::ids::{
    AgentId, AgentProfileId, SandboxId, SandboxProfileId, SubjectId, WorkspaceId,
    WorkspaceProfileId, WorktreeId,
};
use layer2_core::profiles::store::{
    AgentProfileStore, ProfilePageRequest, ProfileStoreError, WorkspaceProfileStore,
};
use layer2_core::profiles::{
    AgentProfile, AgentProfileContent, AgentProfileSnapshot, AgentProvider, ModelAccess,
    PermissionLevel, ProfileVisibility, SandboxProfileContent, SandboxProfileSnapshot,
    ThinkingEffort, WorkspaceLocation, WorkspaceProfile, WorkspaceProfileContent,
    WorkspaceProfileRef, WorkspaceProfileSnapshot,
};
use layer2_core::sandbox::store::{
    AgentAdmission, AgentAdmissionResult, AgentAdmissionStore, AgentConnectionRecoveryCause,
    AgentConnectionRecoveryOutcome, AgentConnectionRecoveryRequest, AgentControlResult,
    AgentControlStore, AgentControlStoreError, AgentControlTarget, AgentConversationMutation,
    AgentConversationStore, AgentDeleteRequest, AgentLogStore, AgentStopRequest, AgentStore,
    AgentWorkMutation, AgentWorkerStore, PageRequest, SandboxAdmission, SandboxAdmissionName,
    SandboxAdmissionResult, SandboxAdmissionStore, SandboxEventStore, SandboxStore,
    SandboxStoreError, SandboxTransaction, SandboxTransactionStore, SandboxWorkerStore,
    VersionedWorktree, WorkspaceAdmission, WorkspaceAdmissionKind, WorkspaceAdmissionResult,
    WorkspaceAdmissionStore, WorkspaceStore, WorkspaceWorkerStore, WorktreeStore, MAX_PAGE_SIZE,
};
use layer2_core::sandbox::{
    Agent, AgentLogEntry, AgentLogKind, AgentOrigin, AgentStatus, AgentStopReason,
    AgentWorkReceipt, AgentWorkState, AgentWorkStep, ConversationState, CreateWorkspace, Sandbox,
    SandboxCredentialAuthority, SandboxCredentialState, SandboxEvent, SandboxEventKind,
    SandboxLimits, SandboxOrigin, SandboxSignInState, SandboxStatus, SandboxWorkState,
    SandboxWorkStep, TypedFailure, Workspace, WorkspaceOrigin, WorkspaceService,
    WorkspaceServiceError, WorkspaceSource, WorkspaceStatus, WorkspaceWorkState, WorkspaceWorkStep,
    Worktree, WorktreeStatus,
};
use layer2_core::{Authorization, Caller, Capability};
use mongodb::bson::{doc, Bson, Document};
use mongodb::event::command::CommandEvent;
use mongodb::event::EventHandler;
use mongodb::options::{ClientOptions, ReadConcern};
use mongodb::Client;

use crate::auth::persistence::SecretValue;
use crate::sandbox::{
    CredentialCustodyError, CredentialMaterial, CredentialRevocation, SandboxCredentialCustody,
};

use super::agent_control_lineage::{fence_control_lineage, load_control_snapshot, LineageError};
use super::agent_control_receipt::{
    read_control_receipts, ControlOperation, ControlRequest, DELETE_RECEIPT_FIELD,
    STOP_RECEIPT_FIELD,
};
use super::agent_control_store::{plan_delete, plan_stop};
use super::agent_control_transaction::{
    control_snapshot_matches, reconciliation_error, same_agent_incarnation,
};
use super::agent_conversation_store::{
    reconcile_conversation, reconcile_conversation_snapshot, validate_mutation, ConversationReceipt,
};
use super::agent_log::log_document;
use super::agent_log_integrity::AppendCapacity;
use super::agent_worker_capacity::publication_capacity;
use super::agent_worker_reconcile::reconcile_publication;
use super::agent_worker_transaction::PublicationReceipt;
use super::agents::agent_replacement_fields;
use super::encryption::AuthEncryptionKey;
use super::sandbox_credentials::{
    MongoSandboxCredentialStore, SANDBOX_CREDENTIALS, SANDBOX_CREDENTIAL_REVOCATIONS,
};
use super::sandbox_events::event_document;
use super::sandbox_mongo::{
    is_transient, start_transaction, MongoProfileStore, MongoSandboxStore, AGENTS, AGENT_LOG,
    AGENT_PROFILES, AGENT_PROFILE_REVISIONS, CHECKPOINTS, DOCUMENT_SCHEMA_VERSION, SANDBOXES,
    SANDBOX_ADMISSIONS, SANDBOX_EVENTS, SANDBOX_PROFILES, SANDBOX_PROFILE_REVISIONS, WORKSPACES,
    WORKSPACE_PROFILES, WORKSPACE_PROFILE_REVISIONS, WORKTREES,
};
use super::worktrees::worktree_replacement_fields;
use super::DefaultProfileSpec;

const NOW: &str = "2026-09-05T00:00:00Z";
mod background_checkpoint_tests;
mod command_ledger_tests;
mod conversation_fixtures;
mod live_sandbox_pages;
pub(in crate::store) mod resource_access_tests;
mod visible_inventory;
use conversation_fixtures::*;
const MINUTE: &str = "2026-09-05T00:01:00Z";
const TWO_MINUTES: &str = "2026-09-05T00:02:00Z";
static DATABASE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
#[derive(Clone, Default)]
struct CommandTrace {
    started: Vec<(String, Document)>,
    failed_commits: usize,
    commit_replies: Vec<Document>,
}

async fn database(prefix: &str) -> (Client, String) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(uri)
        .await
        .expect("test Mongo connects");
    let sequence = DATABASE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let name = format!("cma_804_{prefix}_{}_{sequence}", std::process::id());
    (client, name)
}

fn command_uses_snapshot_read(command: &Document) -> bool {
    command
        .get_document("readConcern")
        .ok()
        .and_then(|concern| concern.get_str("level").ok())
        == Some("snapshot")
}

pub(super) fn sandbox(id: &str) -> Sandbox {
    Sandbox {
        id: SandboxId(id.into()),
        owner: SubjectId("owner".into()),
        name: "research".into(),
        origin: SandboxOrigin::Manual,
        profile: SandboxProfileSnapshot {
            id: SandboxProfileId("sbp_test".into()),
            revision: 1,
            digest: "a".repeat(64),
            content: SandboxProfileContent {
                runtime_image: None,
                cpu_millis: 1_000,
                memory_mib: 2_048,
                nyxid_services: vec![],
                agent_provider: AgentProvider::Codex,
                model_access: ModelAccess::ChronoaiManaged,
                max_workspaces: 4,
                max_agents_per_workspace: 4,
                workspace_profiles: vec![],
                software_install_commands: vec![],
                environment_variables: vec![],
            },
        },
        status: SandboxStatus::Active,
        failure: None,
        runtime: None,
        credential: SandboxCredentialState {
            source: Default::default(),
            service_grants: Vec::new(),
            service_scope: None,
            reaches: vec![],
            key_generation: 1,
            delivered: true,
            current: Some(SandboxCredentialAuthority {
                generation: 1,
                provider_key_id: "provider-key".into(),
                provider_state_version: 1,
                issued_at: NOW.into(),
                expires_at: "2026-10-05T00:00:00Z".into(),
                grant_fingerprint: "a".repeat(64),
            }),
            pending: None,
            revoke_debt: vec![],
            sign_in: SandboxSignInState::NotRequired,
        },
        work: SandboxWorkState::new(SandboxWorkStep::WaitForCredentialRotation, NOW.into()),
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

fn provisioning_sandbox(id: &str) -> Sandbox {
    let mut sandbox = sandbox(id);
    sandbox.status = SandboxStatus::Provisioning;
    sandbox.credential.key_generation = 0;
    sandbox.credential.delivered = false;
    sandbox.credential.current = None;
    sandbox.work = SandboxWorkState::new(SandboxWorkStep::ProvisionRuntime, NOW.into());
    sandbox
}

fn admission(id: &str, limit: u16) -> SandboxAdmission {
    SandboxAdmission::new(
        provisioning_sandbox(id),
        SandboxAdmissionName::Allocate("research".into()),
        limit,
        NOW.into(),
    )
    .unwrap()
}

fn retired_sandbox(id: &str) -> Sandbox {
    let mut sandbox = sandbox(id);
    sandbox.status = SandboxStatus::Retired;
    sandbox.credential.delivered = false;
    sandbox.credential.current = None;
    sandbox.work = SandboxWorkState::new(SandboxWorkStep::Purge, NOW.into());
    sandbox.retired_at = Some(NOW.into());
    sandbox.purge_after = Some(NOW.into());
    sandbox
}

fn credential(sandbox_id: &str, plaintext: &str) -> CredentialMaterial {
    CredentialMaterial {
        source: Default::default(),
        sandbox_id: SandboxId(sandbox_id.into()),
        owner: SubjectId("owner".into()),
        authority: SandboxCredentialAuthority {
            generation: 1,
            provider_key_id: "provider-key".into(),
            provider_state_version: 1,
            issued_at: NOW.into(),
            expires_at: "2026-10-05T00:00:00Z".into(),
            grant_fingerprint: "c".repeat(64),
        },
        value: SecretValue::new(plaintext.into()),
    }
}

fn encryption_key() -> AuthEncryptionKey {
    AuthEncryptionKey::from_base64("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=")
        .expect("test encryption key")
}

fn agent_profile(id: &str) -> AgentProfile {
    AgentProfile {
        internal: false,
        id: AgentProfileId(id.into()),
        owner: SubjectId("owner".into()),
        name: "Research agent".into(),
        description: String::new(),
        visibility: ProfileVisibility::Private,
        forked_from: None,
        uses_count: 0,
        draft: AgentProfileContent {
            isolated_agent: false,
            model: None,
            thinking_effort: ThinkingEffort::Medium,
            permission_level: PermissionLevel::Guarded,
            skills: vec![],
            system_prompt: String::new(),
        },
        draft_version: 1,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        retired_at: None,
    }
}

fn workspace_service_profile(id: &str, name: &str) -> WorkspaceProfile {
    WorkspaceProfile {
        internal: false,
        id: WorkspaceProfileId(id.into()),
        owner: SubjectId("owner".into()),
        name: format!("{name} profile"),
        description: "Workspace admission integration profile".into(),
        visibility: ProfileVisibility::Private,
        forked_from: None,
        uses_count: 0,
        draft: WorkspaceProfileContent {
            location: WorkspaceLocation::Directory { name: name.into() },
            name: name.into(),
            setup_commands: vec![],
            agent_profiles: vec![],
        },
        draft_version: 1,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        retired_at: None,
    }
}

fn worktree_and_agent(sandbox_id: &SandboxId) -> (Worktree, Agent) {
    let agent_id = AgentId("agt_atomic".into());
    let worktree = Worktree {
        id: WorktreeId("wkt_atomic".into()),
        workspace_id: WorkspaceId("wks_atomic".into()),
        sandbox_id: sandbox_id.clone(),
        owner: SubjectId("owner".into()),
        name: "wt-1".into(),
        path: "/home/cma/project/.worktrees/wt-1".into(),
        branch: "cma/wt-1".into(),
        base_commit: "0123456789abcdef".into(),
        status: WorktreeStatus::Ready,
        agent_id: Some(agent_id.clone()),
        revision: 1,
    };
    let agent = Agent {
        id: agent_id,
        worktree_id: worktree.id.clone(),
        workspace_id: worktree.workspace_id.clone(),
        sandbox_id: sandbox_id.clone(),
        owner: worktree.owner.clone(),
        creation_key: "create-atomic".into(),
        creation_fingerprint: "d".repeat(64),
        profile: AgentProfileSnapshot {
            id: AgentProfileId("agp_test".into()),
            revision: 1,
            digest: "b".repeat(64),
            content: agent_profile("agp_test").draft,
        },
        capabilities: layer2_core::RuntimeCapabilities::new([
            layer2_core::RuntimeCapability::Conversation,
        ]),
        status: AgentStatus::Starting,
        conversation: ConversationState {
            preferences: Default::default(),
            managed_guidance_revision: 0,
            observations: Default::default(),
            settings: None,
            conversation_id: None,
            runtime_generation: None,
            last_seq: 0,
            first_message: None,
            active_turn: None,
            operation: None,
            queued_messages: vec![],
        },
        work: AgentWorkState::new(AgentWorkStep::AddWorktree, NOW.into()),
        failure: None,
        title: None,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        ready_at: None,
        stopped_at: None,
        requested_stop_reason: None,
        stop_reason: None,
        deleted_at: None,
        origin: AgentOrigin::Manual,
        revision: 1,
    };
    (worktree, agent)
}

fn workspace(sandbox: &Sandbox, id: &str, name: &str) -> Workspace {
    Workspace {
        id: WorkspaceId(id.into()),
        sandbox_id: sandbox.id.clone(),
        owner: sandbox.owner.clone(),
        origin: WorkspaceOrigin::Manual,
        profile: WorkspaceProfileSnapshot {
            id: WorkspaceProfileId("wkp_mongo".into()),
            revision: 1,
            digest: "d".repeat(64),
            content: WorkspaceProfileContent {
                location: WorkspaceLocation::Directory { name: name.into() },
                name: name.into(),
                setup_commands: vec![],
                agent_profiles: vec![],
            },
        },
        name: name.into(),
        path: format!("/home/cma/{name}"),
        source: WorkspaceSource::Empty,
        resolved_commit: None,
        status: WorkspaceStatus::Preparing,
        work: WorkspaceWorkState::new(WorkspaceWorkStep::Prepare, NOW.into()),
        next_worktree_number: 1,
        created_at: NOW.into(),
        ready_at: None,
        failed: None,
        removed_at: None,
        revision: 1,
    }
}

fn workspace_admission(workspace: Workspace) -> WorkspaceAdmission {
    WorkspaceAdmission {
        binding: None,
        workspace,
        kind: WorkspaceAdmissionKind::Manual,
        event_at: NOW.into(),
    }
}

fn ready_workspace(parent: &Sandbox, id: &str, name: &str) -> Workspace {
    let mut workspace = workspace(parent, id, name);
    workspace.resolved_commit = Some("a".repeat(40));
    workspace.status = WorkspaceStatus::Ready;
    workspace.work = WorkspaceWorkState::new(WorkspaceWorkStep::None, NOW.into());
    workspace.ready_at = Some(NOW.into());
    workspace
}

fn agent_admission(workspace: &Workspace, key: &str, id: &str) -> AgentAdmission {
    AgentAdmission {
        binding: None,
        created_by: None,
        owner: workspace.owner.clone(),
        workspace_id: workspace.id.clone(),
        agent_id: AgentId(format!("agt_{id}")),
        worktree_id: WorktreeId(format!("wkt_{id}")),
        profile: AgentProfileSnapshot {
            id: AgentProfileId("agp_agent_admission".into()),
            revision: 3,
            digest: "c".repeat(64),
            content: agent_profile("agp_agent_admission").draft,
        },
        capabilities: layer2_core::RuntimeCapabilities::new([
            layer2_core::RuntimeCapability::Conversation,
        ]),
        title: None,
        first_message: None,
        origin: AgentOrigin::Manual,
        creation_key: key.into(),
        event_at: NOW.into(),
    }
}

fn created_agent(result: AgentAdmissionResult) -> layer2_core::sandbox::store::AgentCreation {
    match result {
        AgentAdmissionResult::Created(creation) => creation,
        AgentAdmissionResult::Existing(_) => panic!("expected a fresh agent admission"),
    }
}

fn agent_for_worker(id: &str, ready_at: &str) -> Agent {
    let (_, mut agent) = worktree_and_agent(&SandboxId("sbx_agent_worker".into()));
    agent.id = AgentId(id.into());
    agent.worktree_id = WorktreeId(format!("wkt_{id}"));
    agent.creation_key = format!("create-{id}");
    agent.work.ready_at = ready_at.into();
    agent
}

async fn claimed_agent_work_fixture(
    store: &MongoSandboxStore,
    suffix: &str,
) -> (Sandbox, Worktree, Agent) {
    let mut parent = sandbox(&format!("sbx_agent_work_{suffix}"));
    parent.name = suffix.into();
    store.create_sandbox(parent.clone()).await.unwrap();
    let workspace = ready_workspace(
        &parent,
        &format!("wks_agent_work_{suffix}"),
        &format!("agent-work-{suffix}"),
    );
    store.create_workspace(workspace).await.unwrap();
    let (mut worktree, mut agent) = worktree_and_agent(&parent.id);
    let agent_id = AgentId(format!("agt_agent_work_{suffix}"));
    worktree.id = WorktreeId(format!("wkt_agent_work_{suffix}"));
    worktree.workspace_id = WorkspaceId(format!("wks_agent_work_{suffix}"));
    worktree.agent_id = Some(agent_id.clone());
    worktree.status = WorktreeStatus::Creating;
    agent.id = agent_id;
    agent.worktree_id = worktree.id.clone();
    agent.workspace_id = worktree.workspace_id.clone();
    agent.creation_key = format!("create-agent-work-{suffix}");
    store.create_worktree(worktree.clone()).await.unwrap();
    store.create_agent(agent.clone()).await.unwrap();
    let claimed = store
        .claim_agent_work(&agent.id, "worker", NOW.into(), MINUTE.into())
        .await
        .unwrap()
        .unwrap();
    (parent, worktree, claimed)
}

fn ready_publication_claim(mut agent: Agent, conversation_id: &str) -> Agent {
    agent.work.step = AgentWorkStep::PublishReady;
    agent.conversation.conversation_id = Some(conversation_id.into());
    agent.conversation.runtime_generation = Some(1);
    agent.work.receipts = [
        (
            AgentWorkStep::AddWorktree,
            format!("worktree:{}:add:v1", agent.worktree_id),
        ),
        (
            AgentWorkStep::MaterializeHome,
            format!("agent:{}:home:create:v1", agent.id),
        ),
        (
            AgentWorkStep::OpenConversation,
            format!("agent:{}:open:v1", agent.id),
        ),
    ]
    .into_iter()
    .map(|(step, operation_id)| AgentWorkReceipt {
        step,
        operation_id,
        request_sha256: "a".repeat(64),
        completed_at: NOW.into(),
    })
    .collect();
    agent
}

async fn replace_agent_fixture(store: &MongoSandboxStore, agent: &Agent) {
    let result = store
        .agents
        .update_one(
            doc! { "_id": agent.id.as_str() },
            doc! { "$set": agent_replacement_fields(agent).unwrap() },
        )
        .await
        .unwrap();
    assert_eq!(result.matched_count, 1);
}

async fn replace_worktree_fixture(store: &MongoSandboxStore, worktree: &Worktree) {
    let result = store
        .worktrees
        .update_one(
            doc! { "_id": worktree.id.as_str() },
            doc! { "$set": worktree_replacement_fields(worktree).unwrap() },
        )
        .await
        .unwrap();
    assert_eq!(result.matched_count, 1);
}

async fn ready_conversation_fixture(store: &MongoSandboxStore, suffix: &str) -> Agent {
    let (_, mut worktree, claimed) = claimed_agent_work_fixture(store, suffix).await;
    let conversation_id = format!("conversation-{suffix}");
    let claimed = ready_publication_claim(claimed, &conversation_id);
    replace_agent_fixture(store, &claimed).await;
    let fence = claimed.work.lease.as_ref().unwrap().fence;
    let mut ready = claimed.clone();
    ready
        .work
        .advance(
            "worker",
            fence,
            AgentWorkStep::PublishConnection,
            NOW.into(),
        )
        .unwrap();
    ready
        .transition(AgentStatus::Ready, NOW.into(), None)
        .unwrap();
    let log_kinds = vec![
        AgentLogKind::ConversationOpened {
            conversation_id,
            runtime_generation: 1,
        },
        AgentLogKind::StatusChanged {
            from: AgentStatus::Starting,
            to: AgentStatus::Ready,
        },
    ];
    ready.conversation.last_seq = u64::try_from(log_kinds.len()).unwrap();
    let expected_worktree_revision = worktree.revision;
    worktree.transition(WorktreeStatus::Ready).unwrap();
    store
        .commit_agent_work(AgentWorkMutation {
            expected_agent_revision: claimed.revision,
            holder: "worker".into(),
            fence,
            agent: ready.clone(),
            worktree: Some(VersionedWorktree {
                expected_revision: expected_worktree_revision,
                worktree,
            }),
            appended_at: NOW.into(),
            sandbox_event: Some(SandboxEventKind::AgentStatusChanged {
                agent_id: ready.id.clone(),
                from: AgentStatus::Starting,
                to: AgentStatus::Ready,
            }),
            agent_logs: log_kinds,
        })
        .await
        .unwrap();
    ready
}

fn advance_claimed_agent(claimed: &Agent) -> Agent {
    let fence = claimed.work.lease.as_ref().unwrap().fence;
    let mut next = claimed.clone();
    next.work
        .advance("worker", fence, AgentWorkStep::MaterializeHome, NOW.into())
        .unwrap();
    next.revision += 1;
    next
}

mod agent_log_read_tests;
mod agent_title_tests;
mod agent_transaction_tests;
mod agent_worker_tests;
mod checkpoint_capture_tests;
mod execution_tests;
mod launch_command_tests;
mod profile_transaction_tests;
mod sandbox_lifecycle_tests;
mod service_access_tests;
mod skill_material_tests;
mod transaction_reconciliation_tests;
mod warm_hierarchy_tests;
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn fresh_database_initializes_every_schema_v1_collection_and_index() {
    let (client, name) = database("fresh").await;
    let sandbox_store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let profile_store = MongoProfileStore::open(&client, &name).await.unwrap();
    let _credential_store =
        MongoSandboxCredentialStore::open(&client, &name, "test", &encryption_key())
            .await
            .unwrap();
    let names = client
        .database(&name)
        .list_collection_names()
        .await
        .unwrap();
    for required in [
        SANDBOXES,
        WORKSPACES,
        WORKTREES,
        AGENTS,
        AGENT_LOG,
        SANDBOX_EVENTS,
        SANDBOX_ADMISSIONS,
        SANDBOX_CREDENTIALS,
        SANDBOX_CREDENTIAL_REVOCATIONS,
        CHECKPOINTS,
        AGENT_PROFILES,
        AGENT_PROFILE_REVISIONS,
        WORKSPACE_PROFILES,
        WORKSPACE_PROFILE_REVISIONS,
        SANDBOX_PROFILES,
        SANDBOX_PROFILE_REVISIONS,
    ] {
        assert!(
            names.iter().any(|name| name == required),
            "missing {required}"
        );
    }
    assert!(sandbox_store
        .sandboxes
        .list_index_names()
        .await
        .unwrap()
        .contains(&"sandboxes_owner_name_live".to_string()));
    assert!(sandbox_store
        .agents
        .list_index_names()
        .await
        .unwrap()
        .contains(&"agents_owner_creation_key".to_string()));
    assert!(profile_store
        .agent_revisions
        .list_index_names()
        .await
        .unwrap()
        .contains(&"agent_profile_revisions_profile_revision".to_string()));
    assert!(client
        .database(&name)
        .collection::<Document>(SANDBOX_CREDENTIALS)
        .list_index_names()
        .await
        .unwrap()
        .contains(&"sandbox_credentials_generation".to_string()));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn concurrent_admission_serializes_the_limit_and_default_name_allocation() {
    let (client, name) = database("admission").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let results = futures_util::future::join_all((0..6).map(|index| {
        let store = store.clone();
        async move {
            store
                .admit_sandbox(admission(&format!("sbx_admission_{index}"), 3))
                .await
        }
    }))
    .await;
    let mut admitted = Vec::new();
    for result in results {
        match result {
            Ok(SandboxAdmissionResult::Created(sandbox)) => admitted.push(sandbox),
            Err(SandboxStoreError::LimitReached { limit: 3, live: 3 }) => {}
            other => panic!("unexpected concurrent admission result: {other:?}"),
        }
    }
    assert_eq!(admitted.len(), 3);
    let mut names = admitted
        .iter()
        .map(|sandbox| sandbox.name.as_str())
        .collect::<Vec<_>>();
    names.sort_unstable();
    assert_eq!(names, ["research", "research-2", "research-3"]);
    for sandbox in admitted {
        let events = store.sandbox_events(&sandbox.id, 0, 10).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].seq, 1);
        assert_eq!(events[0].kind, SandboxEventKind::Created);
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn concurrent_workspace_admission_serializes_limits_and_live_names() {
    let (client, name) = database("workspace_admission").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut parent = sandbox("sbx_workspace_admission");
    parent.limits.max_workspaces = 2;
    store.create_sandbox(parent.clone()).await.unwrap();

    let results = futures_util::future::join_all((0..6).map(|index| {
        let store = store.clone();
        let admission = workspace_admission(workspace(
            &parent,
            &format!("wks_concurrent_{index}"),
            &format!("project-{index}"),
        ));
        async move { store.admit_workspace(admission).await }
    }))
    .await;
    let mut admitted = Vec::new();
    for result in results {
        match result {
            Ok(WorkspaceAdmissionResult::Created(workspace)) => admitted.push(workspace),
            Err(SandboxStoreError::WorkspaceLimitReached { limit: 2, live: 2 }) => {}
            other => panic!("unexpected workspace admission result: {other:?}"),
        }
    }
    assert_eq!(admitted.len(), 2);
    assert_eq!(
        store
            .list_workspaces(&parent.id, &Default::default())
            .await
            .unwrap()
            .items
            .len(),
        2
    );
    assert_eq!(
        store.sandbox_events(&parent.id, 0, 10).await.unwrap().len(),
        2
    );

    let mut other = sandbox("sbx_workspace_name");
    other.name = "workspace-name-race".into();
    store.create_sandbox(other.clone()).await.unwrap();
    let left = store.admit_workspace(workspace_admission(workspace(
        &other,
        "wks_name_left",
        "same-name",
    )));
    let right = store.admit_workspace(workspace_admission(workspace(
        &other,
        "wks_name_right",
        "same-name",
    )));
    let (left, right) = tokio::join!(left, right);
    let results = [left, right];
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Ok(WorkspaceAdmissionResult::Created(_))))
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(SandboxStoreError::WorkspaceNameConflict)))
            .count(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn concurrent_agent_admission_at_limit_minus_one_admits_exactly_one() {
    let (client, name) = database("agent_admission_limit").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut parent = sandbox("sbx_agent_admission_limit");
    parent.limits.max_agents_per_workspace = 2;
    parent.profile.content.max_agents_per_workspace = 2;
    store.create_sandbox(parent.clone()).await.unwrap();
    let workspace = ready_workspace(&parent, "wks_agent_admission_limit", "agent-limit");
    store.create_workspace(workspace.clone()).await.unwrap();
    store
        .admit_agent(agent_admission(&workspace, "first", "limit_first"))
        .await
        .unwrap();

    let left = store.admit_agent(agent_admission(&workspace, "left", "limit_left"));
    let right = store.admit_agent(agent_admission(&workspace, "right", "limit_right"));
    let (left, right) = tokio::join!(left, right);
    let results = [left, right];
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Ok(AgentAdmissionResult::Created(_))))
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(
                result,
                Err(SandboxStoreError::AgentLimitReached { limit: 2, live: 2 })
            ))
            .count(),
        1
    );
    let stored_workspace = store.get_workspace(&workspace.id).await.unwrap().unwrap();
    assert_eq!(stored_workspace.next_worktree_number, 3);
    assert_eq!(stored_workspace.revision, 3);
    assert_eq!(store.agents.count_documents(doc! {}).await.unwrap(), 2);
    assert_eq!(store.worktrees.count_documents(doc! {}).await.unwrap(), 2);
    assert_eq!(store.agent_log.count_documents(doc! {}).await.unwrap(), 2);
    assert_eq!(
        store.sandbox_events(&parent.id, 0, 10).await.unwrap().len(),
        4
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn agent_admission_allocations_are_monotonic_and_never_reused() {
    let (client, name) = database("agent_admission_numbers").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut parent = sandbox("sbx_agent_admission_numbers");
    parent.limits.max_agents_per_workspace = 1;
    parent.profile.content.max_agents_per_workspace = 1;
    store.create_sandbox(parent.clone()).await.unwrap();
    let workspace = ready_workspace(&parent, "wks_agent_admission_numbers", "agent-numbers");
    store.create_workspace(workspace.clone()).await.unwrap();

    let mut first = created_agent(
        store
            .admit_agent(agent_admission(&workspace, "number-1", "number_1"))
            .await
            .unwrap(),
    );
    first.agent.status = first.agent.status.transition(AgentStatus::Stopped).unwrap();
    first.agent.revision += 1;
    store.save_agent(1, first.agent.clone()).await.unwrap();
    let mut second = created_agent(
        store
            .admit_agent(agent_admission(&workspace, "number-2", "number_2"))
            .await
            .unwrap(),
    );
    second.agent.status = second
        .agent
        .status
        .transition(AgentStatus::Stopped)
        .unwrap();
    second.agent.revision += 1;
    store.save_agent(1, second.agent.clone()).await.unwrap();
    let third = created_agent(
        store
            .admit_agent(agent_admission(&workspace, "number-3", "number_3"))
            .await
            .unwrap(),
    );

    assert_eq!(first.worktree.name, "wt-1");
    assert_eq!(second.worktree.name, "wt-2");
    assert_eq!(third.worktree.name, "wt-3");
    assert_eq!(second.worktree.branch, "cma/wt-2");
    assert_eq!(
        third.worktree.path,
        "/home/cma/agent-numbers/.worktrees/wt-3"
    );
    assert_eq!(
        store
            .get_workspace(&workspace.id)
            .await
            .unwrap()
            .unwrap()
            .next_worktree_number,
        4
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn agent_admission_replays_exact_keys_and_rejects_changed_payloads() {
    let (client, name) = database("agent_admission_replay").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let parent = sandbox("sbx_agent_admission_replay");
    store.create_sandbox(parent.clone()).await.unwrap();
    let workspace = ready_workspace(&parent, "wks_agent_admission_replay", "agent-replay");
    store.create_workspace(workspace.clone()).await.unwrap();

    let request = agent_admission(&workspace, "stable-key", "replay_first");
    let first = created_agent(store.admit_agent(request).await.unwrap());
    let replay = agent_admission(&workspace, "stable-key", "replay_retry");
    assert_eq!(
        store.admit_agent(replay).await.unwrap(),
        AgentAdmissionResult::Existing(first.clone())
    );
    let mut changed = agent_admission(&workspace, "stable-key", "replay_changed");
    changed.title = Some("different immutable payload".into());
    assert_eq!(
        store.admit_agent(changed).await,
        Err(SandboxStoreError::AgentCreationConflict)
    );
    assert_eq!(store.agents.count_documents(doc! {}).await.unwrap(), 1);
    assert_eq!(store.worktrees.count_documents(doc! {}).await.unwrap(), 1);
    assert_eq!(store.agent_log.count_documents(doc! {}).await.unwrap(), 1);
    assert_eq!(
        store
            .get_workspace(&workspace.id)
            .await
            .unwrap()
            .unwrap()
            .next_worktree_number,
        2
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn late_agent_admission_failure_rolls_back_every_artifact() {
    let (client, name) = database("agent_admission_rollback").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let parent = sandbox("sbx_agent_admission_rollback");
    store.create_sandbox(parent.clone()).await.unwrap();
    let workspace = ready_workspace(&parent, "wks_agent_admission_rollback", "agent-rollback");
    store.create_workspace(workspace.clone()).await.unwrap();
    let conflicting = SandboxEvent {
        sandbox_id: parent.id.clone(),
        seq: 2,
        appended_at: NOW.into(),
        kind: SandboxEventKind::Created,
    };
    store
        .sandbox_events
        .insert_one(event_document(&conflicting).unwrap())
        .await
        .unwrap();
    let request = agent_admission(&workspace, "will-roll-back", "rollback");

    assert_eq!(
        store.admit_agent(request.clone()).await,
        Err(SandboxStoreError::SequenceConflict)
    );
    assert_eq!(
        store.get_workspace(&workspace.id).await.unwrap(),
        Some(workspace)
    );
    assert_eq!(
        store.get_sandbox(&parent.id).await.unwrap(),
        Some(parent.clone())
    );
    let parent_row = store
        .sandboxes
        .find_one(doc! { "_id": parent.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(parent_row.get_i64("last_seq").unwrap(), 0);
    assert_eq!(store.get_agent(&request.agent_id).await.unwrap(), None);
    assert_eq!(
        store.get_worktree(&request.worktree_id).await.unwrap(),
        None
    );
    assert_eq!(store.agent_log.count_documents(doc! {}).await.unwrap(), 0);
    assert_eq!(
        store.sandbox_events.count_documents(doc! {}).await.unwrap(),
        1
    );
    assert!(store
        .sandbox_events
        .find_one(doc! { "sandbox_id": parent.id.as_str(), "seq": 1_i64 })
        .await
        .unwrap()
        .is_none());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn sandbox_profile_workspace_origin_replays_the_existing_admission() {
    let (client, name) = database("workspace_origin").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut parent = sandbox("sbx_workspace_origin");
    parent.status = SandboxStatus::Installing;
    let mut first = workspace(&parent, "wks_origin_first", "profile-project");
    first.origin = WorkspaceOrigin::SandboxProfile { index: 0 };
    parent.profile.content.workspace_profiles = vec![WorkspaceProfileRef::new(
        first.profile.id.clone(),
        first.profile.revision,
    )];
    store.create_sandbox(parent.clone()).await.unwrap();

    let created = store
        .admit_workspace(WorkspaceAdmission {
            binding: None,
            workspace: first.clone(),
            kind: WorkspaceAdmissionKind::SandboxProfile,
            event_at: NOW.into(),
        })
        .await
        .unwrap();
    assert_eq!(created, WorkspaceAdmissionResult::Created(first.clone()));

    let mut replay = first.clone();
    replay.id = WorkspaceId("wks_origin_replay".into());
    let replayed = store
        .admit_workspace(WorkspaceAdmission {
            binding: None,
            workspace: replay,
            kind: WorkspaceAdmissionKind::SandboxProfile,
            event_at: "2026-09-05T00:00:01Z".into(),
        })
        .await
        .unwrap();
    assert_eq!(replayed, WorkspaceAdmissionResult::Existing(first));
    assert_eq!(
        store.sandbox_events(&parent.id, 0, 10).await.unwrap().len(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn concurrent_sandbox_profile_workspace_replays_create_one_aggregate() {
    let (client, name) = database("workspace_origin_concurrent").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut parent = sandbox("sbx_workspace_origin_concurrent");
    parent.status = SandboxStatus::Installing;
    let mut template = workspace(&parent, "wks_origin_concurrent_0", "concurrent-project");
    template.origin = WorkspaceOrigin::SandboxProfile { index: 0 };
    parent.profile.content.workspace_profiles = vec![WorkspaceProfileRef::new(
        template.profile.id.clone(),
        template.profile.revision,
    )];
    store.create_sandbox(parent.clone()).await.unwrap();

    let results = futures_util::future::join_all((0..8).map(|index| {
        let store = store.clone();
        let mut candidate = template.clone();
        candidate.id = WorkspaceId(format!("wks_origin_concurrent_{index}"));
        async move {
            store
                .admit_workspace(WorkspaceAdmission {
                    binding: None,
                    workspace: candidate,
                    kind: WorkspaceAdmissionKind::SandboxProfile,
                    event_at: NOW.into(),
                })
                .await
        }
    }))
    .await;
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Ok(WorkspaceAdmissionResult::Created(_))))
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Ok(WorkspaceAdmissionResult::Existing(_))))
            .count(),
        7
    );
    let admitted_ids = results
        .into_iter()
        .map(|result| match result.unwrap() {
            WorkspaceAdmissionResult::Created(workspace)
            | WorkspaceAdmissionResult::Existing(workspace) => workspace.id,
        })
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(admitted_ids.len(), 1);
    assert_eq!(
        store
            .list_workspaces(&parent.id, &Default::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    assert_eq!(
        store.sandbox_events(&parent.id, 0, 10).await.unwrap().len(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn workspace_service_records_each_mongo_profile_admission_once() {
    let (client, name) = database("workspace_service_use_count").await;
    let sandbox_store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let profile_store = Arc::new(MongoProfileStore::open(&client, &name).await.unwrap());

    let manual_profile = workspace_service_profile("wkp_mongo_manual", "manual-project");
    profile_store
        .create_workspace_profile(manual_profile.clone())
        .await
        .unwrap();
    let manual_revision = profile_store
        .publish_workspace_profile(&manual_profile.id, 1, NOW.into())
        .await
        .unwrap();
    let manual_reference =
        WorkspaceProfileRef::new(manual_revision.profile_id.clone(), manual_revision.revision);

    let automatic_profile = workspace_service_profile("wkp_mongo_automatic", "auto-project");
    profile_store
        .create_workspace_profile(automatic_profile.clone())
        .await
        .unwrap();
    let automatic_revision = profile_store
        .publish_workspace_profile(&automatic_profile.id, 1, NOW.into())
        .await
        .unwrap();
    let automatic_reference = WorkspaceProfileRef::new(
        automatic_revision.profile_id.clone(),
        automatic_revision.revision,
    );

    let mut manual_parent = sandbox("sbx_workspace_service_manual");
    manual_parent.name = "workspace-service-manual".into();
    sandbox_store
        .create_sandbox(manual_parent.clone())
        .await
        .unwrap();
    let mut automatic_parent = sandbox("sbx_workspace_service_automatic");
    automatic_parent.name = "workspace-service-automatic".into();
    automatic_parent.status = SandboxStatus::Installing;
    automatic_parent.profile.content.workspace_profiles = vec![automatic_reference.clone()];
    sandbox_store
        .create_sandbox(automatic_parent.clone())
        .await
        .unwrap();

    let service = WorkspaceService::new(sandbox_store.clone(), profile_store.clone());
    let owner = Caller::new(
        SubjectId("owner".into()),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    service
        .create(
            &owner,
            &manual_parent.id,
            CreateWorkspace {
                profile: manual_reference.clone(),
                confirmed_digest: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        service
            .create(
                &owner,
                &manual_parent.id,
                CreateWorkspace {
                    profile: manual_reference.clone(),
                    confirmed_digest: None,
                },
            )
            .await
            .unwrap_err(),
        WorkspaceServiceError::NameConflict
    );
    assert_eq!(
        profile_store
            .workspace_profile(&manual_reference.id)
            .await
            .unwrap()
            .unwrap()
            .uses_count,
        1
    );

    let automatic = futures_util::future::join_all(
        (0..8)
            .map(|_| service.create_profile_workspace(&automatic_parent, &automatic_reference, 0)),
    )
    .await;
    let ids = automatic
        .into_iter()
        .map(|result| result.unwrap().id)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(ids.len(), 1);
    assert_eq!(
        profile_store
            .workspace_profile(&automatic_reference.id)
            .await
            .unwrap()
            .unwrap()
            .uses_count,
        1
    );
    assert_eq!(
        sandbox_store
            .list_workspaces(&automatic_parent.id, &Default::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn sandbox_profile_workspace_rejects_a_forged_durable_slot_reference() {
    let (client, name) = database("workspace_origin_forged").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut parent = sandbox("sbx_workspace_origin_forged");
    parent.status = SandboxStatus::Installing;
    parent.profile.content.workspace_profiles = vec![WorkspaceProfileRef::new(
        WorkspaceProfileId("wkp_expected".into()),
        7,
    )];
    store.create_sandbox(parent.clone()).await.unwrap();

    let mut forged = workspace(&parent, "wks_origin_forged", "forged-project");
    forged.origin = WorkspaceOrigin::SandboxProfile { index: 0 };
    assert_eq!(
        store
            .admit_workspace(WorkspaceAdmission {
                binding: None,
                workspace: forged,
                kind: WorkspaceAdmissionKind::SandboxProfile,
                event_at: NOW.into(),
            })
            .await
            .unwrap_err(),
        SandboxStoreError::WorkspaceOriginConflict
    );
    assert!(store
        .list_workspaces(&parent.id, &Default::default())
        .await
        .unwrap()
        .items
        .is_empty());
    assert!(store
        .sandbox_events(&parent.id, 0, 10)
        .await
        .unwrap()
        .is_empty());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn durable_parent_slot_change_rejects_an_obsolete_workspace_replay() {
    let (client, name) = database("workspace_origin_changed").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut stale_parent = sandbox("sbx_workspace_origin_changed");
    stale_parent.status = SandboxStatus::Installing;
    let mut first = workspace(&stale_parent, "wks_origin_obsolete", "obsolete-project");
    first.origin = WorkspaceOrigin::SandboxProfile { index: 0 };
    stale_parent.profile.content.workspace_profiles = vec![WorkspaceProfileRef::new(
        first.profile.id.clone(),
        first.profile.revision,
    )];
    store.create_sandbox(stale_parent.clone()).await.unwrap();
    assert!(matches!(
        store
            .admit_workspace(WorkspaceAdmission {
                binding: None,
                workspace: first.clone(),
                kind: WorkspaceAdmissionKind::SandboxProfile,
                event_at: NOW.into(),
            })
            .await
            .unwrap(),
        WorkspaceAdmissionResult::Created(_)
    ));

    let mut durable_parent = stale_parent.clone();
    durable_parent.profile.content.workspace_profiles = vec![WorkspaceProfileRef::new(
        WorkspaceProfileId("wkp_replacement".into()),
        2,
    )];
    durable_parent.revision += 1;
    store.save_sandbox(1, durable_parent).await.unwrap();

    let mut replay = first;
    replay.id = WorkspaceId("wks_origin_obsolete_replay".into());
    assert_eq!(
        store
            .admit_workspace(WorkspaceAdmission {
                binding: None,
                workspace: replay,
                kind: WorkspaceAdmissionKind::SandboxProfile,
                event_at: "2026-09-05T00:00:01Z".into(),
            })
            .await
            .unwrap_err(),
        SandboxStoreError::WorkspaceOriginConflict
    );
    assert_eq!(
        store
            .list_workspaces(&stale_parent.id, &Default::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    assert_eq!(
        store
            .sandbox_events(&stale_parent.id, 0, 10)
            .await
            .unwrap()
            .len(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn workspace_admission_and_terminal_event_failures_roll_back_the_aggregate() {
    let (client, name) = database("workspace_rollback").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let parent = sandbox("sbx_workspace_rollback");
    store.create_sandbox(parent.clone()).await.unwrap();
    let conflicting = SandboxEvent {
        sandbox_id: parent.id.clone(),
        seq: 1,
        appended_at: NOW.into(),
        kind: SandboxEventKind::Created,
    };
    store
        .sandbox_events
        .insert_one(event_document(&conflicting).unwrap())
        .await
        .unwrap();

    let candidate = workspace(&parent, "wks_admission_rollback", "admission-rollback");
    assert_eq!(
        store
            .admit_workspace(workspace_admission(candidate.clone()))
            .await,
        Err(SandboxStoreError::SequenceConflict)
    );
    assert_eq!(store.get_workspace(&candidate.id).await.unwrap(), None);
    let parent_row = store
        .sandboxes
        .find_one(doc! { "_id": parent.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(parent_row.get_i64("last_seq").unwrap(), 0);
    assert!(!parent_row.contains_key("workspace_admission_fence"));

    let original = workspace(&parent, "wks_terminal_rollback", "terminal-rollback");
    store.create_workspace(original.clone()).await.unwrap();
    let failure = TypedFailure {
        class: "setup_failed".into(),
        detail: "command failed".into(),
    };
    let mut failed = original.clone();
    failed.work.schedule(WorkspaceWorkStep::None, NOW.into());
    failed
        .transition(WorkspaceStatus::Failed, NOW.into(), Some(failure.clone()))
        .unwrap();
    assert_eq!(
        store
            .commit(SandboxTransaction::SaveWorkspaceAndAppend {
                expected_revision: original.revision,
                workspace: Box::new(failed),
                event_at: NOW.into(),
                event: SandboxEventKind::WorkspaceFailed {
                    workspace_id: original.id.clone(),
                    failure,
                },
            })
            .await,
        Err(SandboxStoreError::SequenceConflict)
    );
    assert_eq!(
        store.get_workspace(&original.id).await.unwrap(),
        Some(original)
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn workspace_claims_survive_reopen_and_fence_out_an_expired_worker() {
    let (client, name) = database("workspace_claims").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let parent = sandbox("sbx_workspace_claims");
    store.create_sandbox(parent.clone()).await.unwrap();
    let candidate = workspace(&parent, "wks_claims", "claims-project");
    store.create_workspace(candidate.clone()).await.unwrap();
    drop(store);

    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        store.due_workspace_work(NOW, 10).await.unwrap(),
        vec![candidate.id.clone()]
    );
    let first = store.claim_workspace_work(
        &candidate.id,
        "worker-a",
        NOW.into(),
        "2026-09-05T00:01:00Z".into(),
    );
    let second = store.claim_workspace_work(
        &candidate.id,
        "worker-b",
        NOW.into(),
        "2026-09-05T00:01:00Z".into(),
    );
    let (first, second) = tokio::join!(first, second);
    let claims = [first.unwrap(), second.unwrap()];
    assert_eq!(claims.iter().filter(|claim| claim.is_some()).count(), 1);
    let mut stale = claims.into_iter().flatten().next().unwrap();
    let stale_holder = stale.work.lease.as_ref().unwrap().holder.clone();
    let stale_fence = stale.work.lease.as_ref().unwrap().fence;
    assert_eq!(stale_fence, 1);

    let takeover = store
        .claim_workspace_work(
            &candidate.id,
            "worker-c",
            "2026-09-05T00:01:00Z".into(),
            "2026-09-05T00:02:00Z".into(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(takeover.work.lease.as_ref().unwrap().fence, 2);
    let expected = stale.revision;
    stale
        .work
        .advance(
            &stale_holder,
            stale_fence,
            WorkspaceWorkStep::Setup,
            NOW.into(),
        )
        .unwrap();
    stale.revision += 1;
    assert_eq!(
        store.save_workspace(expected, stale).await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(
        store
            .get_workspace(&candidate.id)
            .await
            .unwrap()
            .unwrap()
            .work
            .lease
            .unwrap()
            .holder,
        "worker-c"
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn workspace_removal_locks_the_row_and_durably_admits_live_child_cleanup() {
    let (client, name) = database("workspace_removal").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let parent = sandbox("sbx_workspace_removal");
    store.create_sandbox(parent.clone()).await.unwrap();
    let candidate = workspace(&parent, "wks_atomic", "removal-project");
    store.create_workspace(candidate.clone()).await.unwrap();
    let (_, mut agent) = worktree_and_agent(&parent.id);
    agent.workspace_id = candidate.id.clone();
    store.create_agent(agent.clone()).await.unwrap();

    let removing = store
        .begin_workspace_removal(&candidate.id, &candidate.owner, NOW.into())
        .await
        .unwrap();
    assert_eq!(removing.status, WorkspaceStatus::Removing);
    assert_eq!(removing.work.step, WorkspaceWorkStep::Remove);
    assert_eq!(removing.revision, 2);
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
    let raw = store
        .workspaces
        .find_one(doc! { "_id": candidate.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(raw.get_i64("removal_lock_fence").unwrap(), 1);
    assert_eq!(
        store
            .begin_workspace_removal(&candidate.id, &candidate.owner, NOW.into())
            .await
            .unwrap(),
        removing
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn claims_are_exclusive_and_an_expired_lease_gets_a_higher_fence() {
    let (client, name) = database("claims").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let sandbox = provisioning_sandbox("sbx_claims");
    store.create_sandbox(sandbox.clone()).await.unwrap();

    let first = store.claim_sandbox_work(
        &sandbox.id,
        "worker-a",
        NOW.into(),
        "2026-09-05T00:01:00Z".into(),
    );
    let second = store.claim_sandbox_work(
        &sandbox.id,
        "worker-b",
        NOW.into(),
        "2026-09-05T00:01:00Z".into(),
    );
    let (first, second) = tokio::join!(first, second);
    let claims = [first.unwrap(), second.unwrap()];
    assert_eq!(claims.iter().filter(|claim| claim.is_some()).count(), 1);
    let claimed = claims.into_iter().flatten().next().unwrap();
    assert_eq!(claimed.work.lease.as_ref().unwrap().fence, 1);
    assert_eq!(claimed.revision, 2);

    let takeover = store
        .claim_sandbox_work(
            &sandbox.id,
            "worker-c",
            "2026-09-05T00:01:00Z".into(),
            "2026-09-05T00:02:00Z".into(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(takeover.work.lease.as_ref().unwrap().holder, "worker-c");
    assert_eq!(takeover.work.lease.as_ref().unwrap().fence, 2);
    assert_eq!(takeover.revision, 3);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn purge_transaction_removes_the_aggregate_hierarchy_events_and_checkpoints() {
    let (client, name) = database("purge").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let sandbox = retired_sandbox("sbx_purge");
    store.create_sandbox(sandbox.clone()).await.unwrap();
    let sandbox_id = sandbox.id.as_str();
    store
        .workspaces
        .insert_one(doc! { "_id": "wks_purge", "sandbox_id": sandbox_id })
        .await
        .unwrap();
    store
        .worktrees
        .insert_one(doc! { "_id": "wkt_purge", "sandbox_id": sandbox_id })
        .await
        .unwrap();
    store
        .agents
        .insert_one(doc! { "_id": "agt_purge", "sandbox_id": sandbox_id })
        .await
        .unwrap();
    store
        .agent_log
        .insert_one(doc! { "_id": "log_purge", "agent_id": "agt_purge" })
        .await
        .unwrap();
    store
        .sandbox_events
        .insert_one(doc! { "_id": "evt_purge", "sandbox_id": sandbox_id })
        .await
        .unwrap();
    store
        .checkpoints
        .insert_one(doc! { "_id": "chk_purge", "sandbox_id": sandbox_id })
        .await
        .unwrap();

    store
        .purge_sandbox(&sandbox.id, sandbox.revision, NOW)
        .await
        .unwrap();
    for collection in [
        &store.sandboxes,
        &store.workspaces,
        &store.worktrees,
        &store.agents,
        &store.agent_log,
        &store.sandbox_events,
        &store.checkpoints,
    ] {
        assert_eq!(
            collection.count_documents(Document::new()).await.unwrap(),
            0
        );
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn encrypted_credential_round_trip_hides_plaintext_and_rejects_tampering() {
    use mongodb::bson::{spec::BinarySubtype, Binary, Bson};

    let (client, name) = database("credentials").await;
    let store =
        MongoSandboxCredentialStore::open(&client, &name, "test-deployment", &encryption_key())
            .await
            .unwrap();
    let expected = credential("sbx_credential", "credential-plaintext");
    store.insert(expected.clone()).await.unwrap();
    let raw = client
        .database(&name)
        .collection::<Document>(SANDBOX_CREDENTIALS);
    let document = raw.find_one(Document::new()).await.unwrap().unwrap();
    assert!(!format!("{document:?}").contains("credential-plaintext"));
    let loaded = store
        .get(&expected.sandbox_id, expected.authority.generation)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.authority, expected.authority);
    assert_eq!(loaded.owner, expected.owner);
    assert_eq!(loaded.value.expose(), "credential-plaintext");

    let revocation = CredentialRevocation {
        sandbox_id: expected.sandbox_id.clone(),
        owner: expected.owner.clone(),
        provider_key_id: "provider-key-loser".into(),
    };
    store.queue_revocation(revocation.clone()).await.unwrap();
    store.queue_revocation(revocation.clone()).await.unwrap();
    assert_eq!(
        store.revocations(&expected.sandbox_id).await.unwrap(),
        vec![revocation.clone()]
    );
    store
        .delete_revocation(&expected.sandbox_id, &revocation.provider_key_id)
        .await
        .unwrap();
    assert!(store
        .revocations(&expected.sandbox_id)
        .await
        .unwrap()
        .is_empty());

    raw.update_one(
        doc! { "_id": document.get_str("_id").unwrap() },
        doc! { "$set": {
            "value.ciphertext": Bson::Binary(Binary {
                subtype: BinarySubtype::Generic,
                bytes: vec![1, 2, 3],
            })
        } },
    )
    .await
    .unwrap();
    assert!(matches!(
        store
            .get(&expected.sandbox_id, expected.authority.generation)
            .await,
        Err(CredentialCustodyError::Corrupt(_))
    ));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn default_profiles_seed_idempotently_and_refuse_configuration_drift() {
    let (client, name) = database("default_profiles").await;
    let store = MongoProfileStore::open(&client, &name).await.unwrap();
    let bounds = layer2_core::profiles::ProfileCapacityBounds {
        min_cpu_millis: 100,
        max_cpu_millis: 8_000,
        min_memory_mib: 128,
        max_memory_mib: 16_384,
    };
    let spec = DefaultProfileSpec {
        cpu_millis: 1_000,
        memory_mib: 1_024,
        agent_model: "model-a".into(),
        agent_effort: ThinkingEffort::High,
    };
    store
        .seed_default_profiles(&spec, bounds, &[])
        .await
        .unwrap();
    store
        .seed_default_profiles(&spec, bounds, &[])
        .await
        .unwrap();

    for collection in [
        &store.agent_profiles,
        &store.workspace_profiles,
        &store.sandbox_profiles,
    ] {
        collection
            .update_many(
                doc! {},
                doc! {"$set": {"uses_count": 7_i64, "record.uses_count": 7_i64}},
            )
            .await
            .unwrap();
    }
    store
        .seed_default_profiles(&spec, bounds, &[])
        .await
        .unwrap();
    for collection in [
        &store.agent_profiles,
        &store.workspace_profiles,
        &store.sandbox_profiles,
    ] {
        assert_eq!(
            collection
                .find_one(doc! {})
                .await
                .unwrap()
                .unwrap()
                .get_i64("uses_count")
                .unwrap(),
            7
        );
    }

    let mut changed = spec;
    changed.agent_model = "model-b".into();
    assert!(matches!(
        store.seed_default_profiles(&changed, bounds, &[]).await,
        Err(layer2_core::StoreError::Malformed(_))
    ));
    assert_eq!(
        store
            .agent_revisions
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn startup_refuses_an_unknown_document_schema_version() {
    let (client, name) = database("unknown_version").await;
    client
        .database(&name)
        .collection::<Document>(SANDBOXES)
        .insert_one(doc! { "_id": "future", "document_schema_version": 2_i64 })
        .await
        .unwrap();
    let error = match MongoSandboxStore::open(&client, &name).await {
        Ok(_) => panic!("an unknown schema version must stop startup"),
        Err(error) => error,
    };
    assert!(matches!(error, layer2_core::StoreError::Malformed(_)));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn unique_names_and_revision_cas_return_typed_errors() {
    let (client, name) = database("cas").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let original = sandbox("sbx_cas_one");
    store.create_sandbox(original.clone()).await.unwrap();
    let mut same_id = original.clone();
    same_id.name = "must-not-replace".into();
    assert_eq!(
        store.create_sandbox(same_id).await,
        Err(SandboxStoreError::Duplicate)
    );
    assert_eq!(
        store.get_sandbox(&original.id).await.unwrap(),
        Some(original.clone())
    );
    assert_eq!(
        store.save_sandbox(u64::MAX, original.clone()).await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(
        store.get_sandbox(&original.id).await.unwrap(),
        Some(original.clone())
    );
    let mut duplicate = original.clone();
    duplicate.id = SandboxId("sbx_cas_two".into());
    assert_eq!(
        store.create_sandbox(duplicate).await,
        Err(SandboxStoreError::Duplicate)
    );
    let mut replacement = original.clone();
    replacement.revision = 2;
    replacement.updated_at = "2026-09-05T00:00:01Z".into();
    store.save_sandbox(1, replacement.clone()).await.unwrap();
    replacement.updated_at = "2026-09-05T00:00:02Z".into();
    assert_eq!(
        store.save_sandbox(1, replacement).await,
        Err(SandboxStoreError::Conflict)
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn injected_event_failure_rolls_back_worktree_agent_and_sequence() {
    let (client, name) = database("atomic").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let sandbox = sandbox("sbx_atomic");
    store.create_sandbox(sandbox.clone()).await.unwrap();
    let conflicting = SandboxEvent {
        sandbox_id: sandbox.id.clone(),
        seq: 1,
        appended_at: NOW.into(),
        kind: SandboxEventKind::Created,
    };
    store
        .sandbox_events
        .insert_one(event_document(&conflicting).unwrap())
        .await
        .unwrap();
    let (worktree, agent) = worktree_and_agent(&sandbox.id);
    let error = store
        .commit(SandboxTransaction::CreateWorktreeAndAgent {
            worktree: Box::new(worktree.clone()),
            agent: Box::new(agent.clone()),
            event_at: NOW.into(),
            event: SandboxEventKind::AgentCreated {
                workspace_id: worktree.workspace_id.clone(),
                worktree_id: worktree.id.clone(),
                agent_id: agent.id.clone(),
            },
        })
        .await
        .unwrap_err();
    assert_eq!(error, SandboxStoreError::SequenceConflict);
    assert_eq!(store.get_worktree(&worktree.id).await.unwrap(), None);
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), None);
    assert_eq!(
        store
            .sandboxes
            .find_one(doc! { "_id": sandbox.id.0 })
            .await
            .unwrap()
            .unwrap()
            .get_i64("last_seq")
            .unwrap(),
        0
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn missing_event_in_the_durable_range_fails_closed() {
    let (client, name) = database("gap").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let sandbox = sandbox("sbx_gap");
    store.create_sandbox(sandbox.clone()).await.unwrap();
    let first = store
        .append_sandbox_event(&sandbox.id, NOW.into(), SandboxEventKind::Created)
        .await
        .unwrap();
    let second = store
        .append_sandbox_event(&sandbox.id, NOW.into(), SandboxEventKind::Created)
        .await
        .unwrap();
    assert_eq!((first.seq, second.seq), (1, 2));
    store
        .sandbox_events
        .delete_one(doc! { "sandbox_id": sandbox.id.as_str(), "seq": 1_i64 })
        .await
        .unwrap();
    assert_eq!(
        store.sandbox_events(&sandbox.id, 0, 10).await,
        Err(SandboxStoreError::SequenceConflict)
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn missing_agent_log_entry_in_the_durable_range_fails_closed() {
    let (client, name) = database("agent_log_gap").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let sandbox = sandbox("sbx_agent_log_gap");
    store.create_sandbox(sandbox.clone()).await.unwrap();
    let (_, agent) = worktree_and_agent(&sandbox.id);
    store.create_agent(agent.clone()).await.unwrap();
    let first = store
        .append_agent_log(
            &agent.id,
            NOW.into(),
            AgentLogKind::Stopped {
                reason: "first".into(),
            },
        )
        .await
        .unwrap();
    let second = store
        .append_agent_log(
            &agent.id,
            NOW.into(),
            AgentLogKind::Stopped {
                reason: "second".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!((first.seq, second.seq), (1, 2));
    assert_eq!(
        store
            .get_agent(&agent.id)
            .await
            .unwrap()
            .unwrap()
            .conversation
            .last_seq,
        2
    );
    let orphan = AgentLogEntry {
        agent_id: agent.id.clone(),
        seq: 3,
        appended_at: NOW.into(),
        kind: AgentLogKind::Deleted,
    };
    store
        .agent_log
        .insert_one(log_document(&orphan).unwrap())
        .await
        .unwrap();
    assert_eq!(
        store.agent_log(&agent.id, 0, 10).await,
        Err(SandboxStoreError::SequenceConflict)
    );
    assert_eq!(
        store.agent_log(&agent.id, 2, 10).await,
        Err(SandboxStoreError::SequenceConflict)
    );
    store
        .agent_log
        .delete_one(doc! { "agent_id": agent.id.as_str(), "seq": 3_i64 })
        .await
        .unwrap();
    store
        .agent_log
        .delete_one(doc! { "agent_id": agent.id.as_str(), "seq": 1_i64 })
        .await
        .unwrap();
    assert_eq!(
        store.agent_log(&agent.id, 0, 10).await,
        Err(SandboxStoreError::SequenceConflict)
    );
    assert_eq!(
        store.agent_log(&agent.id, 1, 10).await,
        Err(SandboxStoreError::SequenceConflict)
    );
    client.database(&name).drop().await.unwrap();
}
