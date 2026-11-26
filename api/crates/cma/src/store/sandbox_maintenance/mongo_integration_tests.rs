use std::sync::Arc;

use layer2_core::ids::{
    AgentId, AgentProfileId, SubjectId, WorkspaceId, WorkspaceProfileId, WorktreeId,
};
use layer2_core::profiles::{
    AgentProfileContent, AgentProfileSnapshot, PermissionLevel, ThinkingEffort, WorkspaceLocation,
    WorkspaceProfileContent, WorkspaceProfileSnapshot,
};
use layer2_core::sandbox::store::{
    AgentAdmission, AgentAdmissionStore, AgentConversationMutation, AgentConversationStore,
    AgentLogStore, AgentStore, SandboxEventStore, SandboxStore, WorkspaceStore,
};
use layer2_core::sandbox::{
    inactivity_pause_notice, Agent, AgentLogEntry, AgentLogKind, AgentOrigin, AgentStatus,
    AgentWorkState, AgentWorkStep, ConversationState, RuntimeBinding, Sandbox, SandboxEvent,
    SandboxEventKind, SandboxLifecycleOperation, SandboxStatus, Workspace, WorkspaceOrigin,
    WorkspaceSource, WorkspaceStatus, WorkspaceWorkState, WorkspaceWorkStep,
};
use mongodb::options::ClientOptions;
use mongodb::Client;

use super::*;
use crate::store::agent_log::log_document;
use crate::store::agents::agent_document;
use crate::store::sandbox_events::event_document;
use crate::store::sandbox_mongo::start_transaction;
use crate::store::sandbox_mongo_integration_tests::sandbox;

const IDLE_AT: &str = "2026-09-05T00:00:00Z";
const LATER: &str = "2026-09-05T00:30:00Z";

#[path = "availability_integration_tests.rs"]
mod availability;
mod contention_completion;
mod contention_tests;
mod desktop_evidence_tests;
mod idle_tests;
mod legacy_log_budget_tests;
mod operator_audit_tests;
#[path = "platform_stop_integration_tests.rs"]
mod platform_stop_integration_tests;
mod policy_tests;
#[path = "restored_connection_integration_tests.rs"]
mod restored_connection_integration_tests;
#[path = "runtime_update_integration_tests.rs"]
mod runtime_update_integration_tests;

pub(crate) async fn database() -> (Client, String, MongoSandboxStore) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(uri).await.unwrap();
    let name = format!("cma_812_maintenance_{}", uuid::Uuid::new_v4().simple());
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    (client, name, store)
}

pub(in crate::store) fn active_sandbox(id: &str) -> Sandbox {
    let mut value = sandbox(id);
    value.name = id.into();
    value.runtime = Some(RuntimeBinding {
        boot_id: None,
        runtime_id: format!("runtime-{id}"),
        runtime_version: "image-v1".into(),
        provision_generation: Some(1),
        bound_at: IDLE_AT.into(),
    });
    value.lifecycle.runtime_generation_high_water = 1;
    value
}

pub(in crate::store) fn ready_agent(sandbox: &Sandbox, id: &str) -> Agent {
    Agent {
        id: AgentId(id.into()),
        worktree_id: WorktreeId(format!("wkt_{id}")),
        workspace_id: WorkspaceId(format!("wks_{}", sandbox.id)),
        sandbox_id: sandbox.id.clone(),
        owner: sandbox.owner.clone(),
        creation_key: format!("create-{id}"),
        creation_fingerprint: "c".repeat(64),
        profile: AgentProfileSnapshot {
            id: AgentProfileId("agp_maintenance".into()),
            revision: 1,
            digest: "d".repeat(64),
            content: AgentProfileContent {
                isolated_agent: false,
                model: None,
                thinking_effort: ThinkingEffort::Medium,
                permission_level: PermissionLevel::Guarded,
                skills: vec![],
                system_prompt: String::new(),
            },
        },
        capabilities: layer2_core::RuntimeCapabilities::new([
            layer2_core::RuntimeCapability::Conversation,
        ]),
        status: AgentStatus::Ready,
        conversation: ConversationState {
            turn_timeout: None,
            preferences: Default::default(),
            managed_guidance_revision: 0,
            observations: Default::default(),
            settings: None,
            conversation_id: Some(format!("conversation-{id}")),
            runtime_generation: Some(1),
            last_seq: 1,
            first_message: None,
            active_turn: None,
            operation: None,
            queued_messages: vec![],
        },
        work: AgentWorkState::new(AgentWorkStep::None, IDLE_AT.into()),
        failure: None,
        title: None,
        created_at: IDLE_AT.into(),
        updated_at: IDLE_AT.into(),
        ready_at: Some(IDLE_AT.into()),
        stopped_at: None,
        requested_stop_reason: None,
        stop_reason: None,
        deleted_at: None,
        origin: AgentOrigin::Manual,
        revision: 1,
    }
}

pub(in crate::store) async fn insert_agent(
    store: &MongoSandboxStore,
    agent: &Agent,
    appended_at: &str,
) {
    store
        .agents
        .insert_one(agent_document(agent).unwrap())
        .await
        .unwrap();
    let entry = AgentLogEntry {
        agent_id: agent.id.clone(),
        seq: 1,
        appended_at: appended_at.into(),
        kind: AgentLogKind::ConversationOpened {
            conversation_id: agent.conversation.conversation_id.clone().unwrap(),
            runtime_generation: 1,
        },
    };
    store
        .agent_log
        .insert_one(log_document(&entry).unwrap())
        .await
        .unwrap();
}

pub(crate) async fn fixture(store: &MongoSandboxStore, id: &str) -> (Sandbox, Agent) {
    let sandbox = active_sandbox(id);
    store.create_sandbox(sandbox.clone()).await.unwrap();
    let agent = ready_agent(&sandbox, &format!("agt_{id}"));
    insert_agent(store, &agent, IDLE_AT).await;
    (sandbox, agent)
}

fn pause(id: &SandboxId) -> SandboxInactivityPauseRequest {
    SandboxInactivityPauseRequest {
        sandbox_id: id.clone(),
        requested_at: LATER.into(),
        pause_after_seconds: 900,
        auto_pause_enabled: false,
        expected_policy_revision: None,
        cause: Default::default(),
    }
}

fn rotation(id: &SandboxId) -> SandboxImageRotationRequest {
    SandboxImageRotationRequest {
        sandbox_id: id.clone(),
        requested_at: LATER.into(),
        expected_runtime_version: "image-v2".into(),
    }
}

#[path = "maintenance_fences_tests.rs"]
mod fences;
#[path = "maintenance_integrity_tests.rs"]
mod integrity;
#[path = "maintenance_retries_tests.rs"]
mod retries;
#[path = "maintenance_scheduling_tests.rs"]
mod scheduling;

#[path = "custom_runtime_mongo_tests.rs"]
mod custom_runtime_mongo_tests;
