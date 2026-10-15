use std::sync::Arc;

use async_trait::async_trait;
use layer2_core::profiles::InMemoryProfileStore;
use layer2_core::sandbox::store::{InMemorySandboxStore, SandboxStore, WorkspaceStore};
use layer2_core::sandbox::{
    AgentLogKind, AgentOperationDelivery, AgentOperationIntent, AgentOperationKind,
    AgentOperationRecord, AgentOperationService, AgentReadService, AgentService, AgentStatus,
    QueuedAgentMessage, Sandbox, SandboxResourceReadError, SandboxResourceReader,
    SandboxResourceReading, WorkspaceStatus, MAX_QUEUED_AGENT_MESSAGES,
};
use layer2_core::{Authorization, Capability, RuntimeCapabilities};

use super::super::{agent_router, agents_tests as agents, instances_tests, AgentRoutesState};
use super::{agent_cases::ContractRuntime, Scenario};

const NOW: &str = "2026-09-05T00:00:00Z";

struct ResourcePort(bool);

#[async_trait]
impl SandboxResourceReader for ResourcePort {
    async fn sandbox_resources(
        &self,
        sandbox: &Sandbox,
    ) -> Result<SandboxResourceReading, SandboxResourceReadError> {
        if self.0 {
            Err(SandboxResourceReadError::Unsupported(
                "contract runtime has no resource observations".into(),
            ))
        } else {
            agents::TestResources.sandbox_resources(sandbox).await
        }
    }
}

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    let kind = name.strip_prefix("agent_state_")?;
    let (status, active_turn) = match kind {
        "no_capabilities" | "resources_unsupported" | "workspace_unready" | "limit" => {
            (AgentStatus::Ready, None)
        }
        "queue_full" => (AgentStatus::Paused, None),
        "steer_ambiguous" | "interrupt_ambiguous" | "working_pending" => {
            (AgentStatus::Working, Some(3))
        }
        "approval_ambiguous" | "approval_pending" => (AgentStatus::WaitingApproval, Some(3)),
        _ => return None,
    };
    let store = Arc::new(InMemorySandboxStore::default());
    let profiles = Arc::new(InMemoryProfileStore::default());
    let registry = Arc::new(ContractRuntime::new(kind.ends_with("_ambiguous")));
    registry.attach_store(store.clone());
    let mut sandbox = agents::sandbox("sbx_route", "owner-a");
    if kind == "limit" {
        sandbox.limits.max_agents_per_workspace = 1;
    }
    store.create_sandbox(sandbox).await.unwrap();
    let mut workspace = agents::workspace("wks_route", "sbx_route", "owner-a");
    if kind == "workspace_unready" {
        workspace.status = WorkspaceStatus::Preparing;
        workspace.resolved_commit = None;
    }
    store.create_workspace(workspace).await.unwrap();
    agents::publish_profile(&profiles).await;
    let capabilities = if kind == "no_capabilities" {
        RuntimeCapabilities::new([])
    } else {
        agents::agent_capabilities()
    };
    let mut logs = if active_turn.is_some() {
        vec![
            AgentLogKind::TurnStarted {
                model: None,
                turn_sequence: 3,
                origin_key: None,
            },
            AgentLogKind::OwnerMessage {
                content: "Review this change".into(),
            },
        ]
    } else {
        vec![]
    };
    if status == AgentStatus::WaitingApproval {
        logs.push(AgentLogKind::ApprovalRequested {
            approval_id: "approval_contract".into(),
            action: Some("write".into()),
            detail: Some("proof.rs".into()),
            runtime_locator: Some("private-runtime-locator".into()),
        });
        logs.push(AgentLogKind::StatusChanged {
            from: AgentStatus::Working,
            to: status,
        });
    }
    let mut agent = agents::seeded_agent("agt_contract", status, 2, active_turn);
    agent.capabilities = capabilities.clone();
    if kind == "queue_full" {
        for index in 0..MAX_QUEUED_AGENT_MESSAGES {
            let content = format!("Queued request {index}");
            agent.conversation.queued_messages.push(
                QueuedAgentMessage::new(index as u64 + 3, content.clone(), NOW.into()).unwrap(),
            );
            logs.push(AgentLogKind::OwnerMessage { content });
        }
    }
    if kind.ends_with("_pending") {
        let sequence = logs.len() as u64 + 3;
        agent.conversation.operation = Some(
            AgentOperationRecord::new(sequence, 1, 3, AgentOperationIntent::Interrupt, NOW.into())
                .unwrap(),
        );
        logs.push(AgentLogKind::OperationDeliveryChanged {
            operation_sequence: sequence,
            operation: AgentOperationKind::Interrupt,
            state: AgentOperationDelivery::DispatchFenced,
        });
    }
    agent.conversation.last_seq = logs.len() as u64 + 2;
    agents::insert_agent(&store, agent, logs).await;
    let router = agent_router(AgentRoutesState::new(
        AgentService::new(store.clone(), profiles, registry.clone(), capabilities),
        AgentReadService::new(
            store.clone(),
            Arc::new(ResourcePort(kind == "resources_unsupported")),
        ),
        AgentOperationService::new(store, registry),
        Arc::new(instances_tests::MemoryOperationStore::default()),
    ));
    Some(Scenario {
        router,
        subject: Some("owner-a"),
        authorization: Authorization::new([Capability::UseOwnSandboxes]),
        symbols: Default::default(),
    })
}
