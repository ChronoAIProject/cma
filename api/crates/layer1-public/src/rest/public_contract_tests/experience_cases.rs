use std::sync::Arc;

use layer2_core::profiles::InMemoryProfileStore;
use layer2_core::sandbox::store::{
    AgentLogStore, AgentStore, InMemorySandboxStore, SandboxLifecycleAgentScope,
    SandboxLifecycleExecutionStore, SandboxLifecycleMutation, SandboxMutationGuard, SandboxStore,
    WorktreeStore,
};
use layer2_core::sandbox::{
    Agent, AgentContextDefaults, AgentContextError, AgentContextService, AgentLogKind,
    AgentOperationDelivery, AgentOperationIntent, AgentOperationKind, AgentOperationRecord,
    AgentService, AgentStatus, AgentStopReason, SandboxEventKind, SandboxPolicy, SandboxService,
    SandboxWorkStep, TypedFailure, WorkspaceService, WorktreeStatus,
};
use layer2_core::{
    Authorization, Capability, PlatformDefaults, RuntimeCapabilities, RuntimeCapability,
};

use super::{agent_cases::ContractRuntime, Scenario};
use crate::rest::agents_tests::{
    self as agents, conversation_fixture, conversation_fixture_with_registry, insert_agent,
    seeded_agent,
};

const NOW: &str = "2026-09-05T00:00:00Z";

struct DefaultsPort {
    pending: bool,
}

#[async_trait::async_trait]
impl AgentContextDefaults for DefaultsPort {
    async fn defaults(&self) -> Result<PlatformDefaults, AgentContextError> {
        if self.pending {
            std::future::pending().await
        } else {
            Err(AgentContextError::Unavailable)
        }
    }
}

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    let (subject, capabilities) = match name {
        "experience_owner"
        | "experience_checkpoint"
        | "experience_agui_admission"
        | "experience_responses_admission"
        | "experience_working"
        | "experience_stopped"
        | "experience_stopped_owner"
        | "experience_stopped_operator"
        | "experience_stopped_runtime_lost"
        | "experience_stopped_sandbox_retired"
        | "experience_stopped_failed"
        | "experience_deleted"
        | "experience_agui_stop_owner"
        | "experience_agui_stop_operator"
        | "experience_agui_stop_runtime_lost"
        | "experience_agui_stop_sandbox_retired"
        | "experience_agui_stop_failed"
        | "experience_agui_failing"
        | "experience_responses_failing"
        | "experience_response_unavailable"
        | "experience_rejected"
        | "experience_responses_rejected"
        | "experience_commands_unavailable"
        | "experience_command_conflict"
        | "experience_no_capabilities"
        | "experience_no_steer"
        | "experience_approval"
        | "experience_pending"
        | "experience_limit"
        | "experience_defaults_unavailable"
        | "experience_timeout_input"
        | "experience_timeout_context"
        | "experience_failed_turn"
        | "experience_agui_ambiguous"
        | "experience_responses_ambiguous" => (Some("owner-a"), vec![Capability::UseOwnSandboxes]),
        "experience_foreign" => (Some("owner-b"), vec![Capability::UseOwnSandboxes]),
        "experience_anonymous" => (None, vec![]),
        "experience_visitor" => (Some("visitor"), vec![Capability::LaunchTriggers]),
        "experience_forbidden" => (Some("owner-a"), vec![]),
        _ => return None,
    };
    let (state, store) = composition(name).await;
    Some(Scenario {
        router: crate::conversation::command_test_support::router(state, store),
        subject,
        authorization: Authorization::new(capabilities),
        symbols: Default::default(),
    })
}

async fn composition(name: &str) -> (crate::AgentConversationState, Arc<InMemorySandboxStore>) {
    let runtime_stop = name
        .strip_prefix("experience_agui_stop_")
        .and_then(stop_reason);
    let (mut state, store) = if name.ends_with("_admission")
        || name.ends_with("_ambiguous")
        || name.ends_with("_acknowledged")
        || name.ends_with("_failing")
        || name == "experience_response_unavailable"
        || name.ends_with("_rejected")
        || runtime_stop.is_some()
    {
        let registry = Arc::new(if let Some(reason) = runtime_stop {
            ContractRuntime::stopping(reason)
        } else if name.ends_with("_failing") {
            ContractRuntime::failing()
        } else if name.ends_with("_rejected") {
            ContractRuntime::rejecting()
        } else if name.ends_with("_admission") {
            ContractRuntime::completing()
        } else {
            ContractRuntime::new(name.ends_with("_ambiguous"))
        });
        let fixture = conversation_fixture_with_registry(registry.clone()).await;
        registry.attach_store(fixture.1.clone());
        fixture
    } else {
        conversation_fixture().await
    };
    let mut logs = vec![
        AgentLogKind::TurnStarted {
            model: None,
            turn_sequence: 3,
            origin_key: Some("contract-private-origin".into()),
        },
        AgentLogKind::OwnerMessage {
            content: "Question".into(),
        },
        AgentLogKind::TurnMetadataSet {
            turn_sequence: 3,
            metadata: [("project".into(), "proof".into())].into(),
        },
        AgentLogKind::AgentMessage {
            content: "First answer".into(),
        },
        AgentLogKind::ToolCallStarted {
            call_id: "call-proof".into(),
            title: "Read proof".into(),
            kind: "read".into(),
            status: "in_progress".into(),
        },
        AgentLogKind::AgentMessage {
            content: "Second answer".into(),
        },
        AgentLogKind::Usage {
            input_tokens: 10,
            output_tokens: 7,
            context_used: 17,
            context_window: Some(200_000),
        },
        AgentLogKind::TurnCompleted { turn_sequence: 3 },
    ];
    let mut agent = if matches!(
        name,
        "experience_working" | "experience_no_steer" | "experience_pending"
    ) {
        logs.truncate(2);
        seeded_agent("agt_contract", AgentStatus::Working, 4, Some(3))
    } else if name == "experience_approval" {
        logs.truncate(2);
        logs.extend([
            AgentLogKind::ApprovalRequested {
                approval_id: "approval_contract".into(),
                action: Some("write".into()),
                detail: Some("proof.rs".into()),
                runtime_locator: Some("private-runtime-locator".into()),
            },
            AgentLogKind::StatusChanged {
                from: AgentStatus::Working,
                to: AgentStatus::WaitingApproval,
            },
        ]);
        seeded_agent("agt_contract", AgentStatus::WaitingApproval, 6, Some(3))
    } else if name == "experience_stopped" {
        seeded_agent("agt_contract", AgentStatus::Stopped, 10, None)
    } else if name == "experience_timeout_input" {
        logs.clear();
        let mut agent = seeded_agent("agt_contract", AgentStatus::Starting, 2, None);
        agent.ready_at = None;
        agent
    } else {
        seeded_agent("agt_contract", AgentStatus::Ready, 10, None)
    };
    if name == "experience_no_capabilities" {
        agent.capabilities = RuntimeCapabilities::new([]);
    } else if name == "experience_no_steer" {
        agent.capabilities = RuntimeCapabilities::new([RuntimeCapability::Conversation]);
    } else if name == "experience_pending" {
        agent.conversation.operation = Some(
            AgentOperationRecord::new(5, 1, 3, AgentOperationIntent::Interrupt, NOW.into())
                .unwrap(),
        );
        logs.push(AgentLogKind::OperationDeliveryChanged {
            operation_sequence: 5,
            operation: AgentOperationKind::Interrupt,
            state: AgentOperationDelivery::DispatchFenced,
        });
        agent.conversation.last_seq = 5;
    } else if name == "experience_failed_turn" {
        *logs.last_mut().unwrap() = AgentLogKind::TurnFailed {
            turn_sequence: 3,
            failure: TypedFailure {
                class: "agent_turn_failed".into(),
                detail: "The runtime could not complete this turn.".into(),
            },
        };
    }
    let agent = insert_agent(&store, agent, logs).await;
    if name == "experience_checkpoint" {
        begin_checkpoint(&store, &agent.sandbox_id).await;
    }
    if name == "experience_response_unavailable" {
        state.reads = super::agent_cases::unavailable_log_reads(store.clone());
    }
    let stopped = if name == "experience_deleted" {
        Some(AgentStopReason::Owner)
    } else {
        name.strip_prefix("experience_stopped_")
            .and_then(stop_reason)
    };
    if let Some(reason) = stopped {
        terminal_history(&store, agent, reason, name == "experience_deleted").await;
    }
    if name == "experience_limit" {
        for index in 1..4 {
            insert_agent(
                &store,
                seeded_agent(
                    &format!("agt_capacity_{index}"),
                    AgentStatus::Ready,
                    2,
                    None,
                ),
                vec![],
            )
            .await;
        }
    }
    if matches!(
        name,
        "experience_defaults_unavailable" | "experience_timeout_context"
    ) {
        let profiles = Arc::new(InMemoryProfileStore::default());
        agents::publish_profile(&profiles).await;
        state.context = AgentContextService::new(
            SandboxService::new(
                store.clone(),
                profiles.clone(),
                SandboxPolicy::default(),
                Arc::new(layer2_core::sandbox::FixedSandboxCredentialPrerequisite::Ready),
            ),
            WorkspaceService::new(store.clone(), profiles.clone()),
            AgentService::new(
                store.clone(),
                profiles.clone(),
                Arc::new(agents::TestRegistry),
                agents::agent_capabilities(),
            ),
            profiles,
            Arc::new(DefaultsPort {
                pending: name == "experience_timeout_context",
            }),
        );
    }
    state = crate::conversation::command_test_support::compose(state, store.clone());
    if name == "experience_commands_unavailable" {
        state.commands = None;
    } else if name == "experience_command_conflict" {
        crate::conversation::command_test_support::reservation_conflict(&mut state, store.clone());
    }
    (state, store)
}

pub(super) async fn begin_checkpoint(store: &InMemorySandboxStore, id: &layer2_core::SandboxId) {
    const CAPTURE_AT: &str = "2026-09-05T00:02:00Z";
    let parent = store.get_sandbox(id).await.unwrap().unwrap();
    let mut sandbox = parent.clone();
    sandbox.begin_checkpoint_capture(CAPTURE_AT.into()).unwrap();
    store
        .commit_sandbox_lifecycle(SandboxLifecycleMutation {
            guard: SandboxMutationGuard::capture(&parent),
            sandbox,
            agents: vec![],
            agent_scope: SandboxLifecycleAgentScope::Touched,
            appended_at: CAPTURE_AT.into(),
            events: vec![SandboxEventKind::WorkStepCompleted {
                step: SandboxWorkStep::RunLifecycleOperation,
            }],
        })
        .await
        .unwrap();
}

fn stop_reason(reason: &str) -> Option<AgentStopReason> {
    match reason {
        "owner" => Some(AgentStopReason::Owner),
        "operator" => Some(AgentStopReason::Operator),
        "runtime_lost" => Some(AgentStopReason::RuntimeLost),
        "sandbox_retired" => Some(AgentStopReason::SandboxRetired),
        "failed" => Some(AgentStopReason::Failed),
        _ => None,
    }
}

async fn terminal_history(
    store: &InMemorySandboxStore,
    mut agent: Agent,
    reason: AgentStopReason,
    deleted: bool,
) {
    assert_eq!(agent.conversation.active_turn, None);
    let revision = agent.revision;
    agent
        .transition(AgentStatus::Stopped, NOW.into(), Some(reason.clone()))
        .unwrap();
    agent.conversation.last_seq += 1;
    store.save_agent(revision, agent.clone()).await.unwrap();
    store
        .append_agent_log(
            &agent.id,
            NOW.into(),
            AgentLogKind::Stopped {
                reason: reason.as_str().into(),
            },
        )
        .await
        .unwrap();
    if !deleted {
        return;
    }
    let mut worktree = store
        .get_worktree(&agent.worktree_id)
        .await
        .unwrap()
        .unwrap();
    for status in [WorktreeStatus::Removing, WorktreeStatus::Removed] {
        let revision = worktree.revision;
        worktree.transition(status).unwrap();
        store
            .save_worktree(revision, worktree.clone())
            .await
            .unwrap();
    }
    let revision = agent.revision;
    agent
        .transition(AgentStatus::Deleted, NOW.into(), None)
        .unwrap();
    agent.conversation.last_seq += 1;
    store.save_agent(revision, agent.clone()).await.unwrap();
    store
        .append_agent_log(&agent.id, NOW.into(), AgentLogKind::Deleted)
        .await
        .unwrap();
}

#[tokio::test]
async fn closing_an_admitted_agui_stream_preserves_explicit_followup_control() {
    use axum::http::{Method, StatusCode};
    use futures_util::StreamExt;
    use layer2_core::{AgentId, Caller, SubjectId};
    use serde_json::json;
    use tower::ServiceExt;
    let (state, store) = composition("experience_agui_acknowledged").await;
    let router = crate::conversation::command_test_support::router(state.clone(), store);
    let response = router.oneshot(crate::rest::agents_tests::request(Method::POST, "/api/v1/agui/run", "owner-a", None,
        Some(json!({"threadId":"agt_contract", "messages":[{"role":"user", "content":"Continue the proof"}]}))))
        .await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.into_body().into_data_stream();
    assert!(stream.next().await.unwrap().is_ok());
    drop(stream);
    let caller = Caller::new(
        SubjectId::new("owner-a"),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let id = AgentId("agt_contract".into());
    state
        .reads
        .read(&caller, &id)
        .await
        .expect("admitted Agent remains readable after stream close");
    state
        .operations
        .steer(&caller, &id, "Focus on the boundary".into())
        .await
        .expect("admitted Agent accepts explicit steering after stream close");
}
