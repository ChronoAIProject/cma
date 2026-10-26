use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use layer2_core::sandbox::store::{AgentStore, InMemorySandboxStore};
use layer2_core::sandbox::{
    AgentLogKind, AgentMessageAssembly, AgentOperationDelivery, AgentRuntimeConnection,
    AgentRuntimeError, AgentRuntimeInputError, AgentRuntimeInputSender, AgentRuntimePromptResponse,
    AgentRuntimeRegistry, AgentRuntimeTarget, AgentRuntimeTurnStopReason, AgentRuntimeUpdate,
    AgentStatus, AgentStopReason, AgentUpdateRecorder, DurableAgentUpdateRecorder,
};
use layer2_core::{AgentId, Authorization, Capability, RuntimeCapabilities, RuntimeCapability};

use super::super::agents_tests as agents;
use super::Scenario;

mod read_failure;
mod stopped;

pub(super) use read_failure::unavailable_log_reads;

#[derive(Clone)]
enum RuntimeOutcome {
    Accept,
    Ambiguous,
    Complete,
    Fail,
    Reject,
    Stop(AgentStopReason),
}

pub(super) struct ContractRuntime {
    store: OnceLock<Arc<InMemorySandboxStore>>,
    outcome: RuntimeOutcome,
}

impl ContractRuntime {
    pub(super) fn new(ambiguous: bool) -> Self {
        Self {
            store: OnceLock::new(),
            outcome: if ambiguous {
                RuntimeOutcome::Ambiguous
            } else {
                RuntimeOutcome::Accept
            },
        }
    }

    pub(super) fn completing() -> Self {
        Self {
            outcome: RuntimeOutcome::Complete,
            ..Self::new(false)
        }
    }

    pub(super) fn failing() -> Self {
        Self {
            outcome: RuntimeOutcome::Fail,
            ..Self::new(false)
        }
    }

    pub(super) fn rejecting() -> Self {
        Self {
            outcome: RuntimeOutcome::Reject,
            ..Self::new(false)
        }
    }

    pub(super) fn stopping(reason: AgentStopReason) -> Self {
        Self {
            outcome: RuntimeOutcome::Stop(reason),
            ..Self::new(false)
        }
    }

    pub(super) fn attach_store(&self, store: Arc<InMemorySandboxStore>) {
        assert!(self.store.set(store).is_ok());
    }
}

#[async_trait]
impl AgentRuntimeRegistry for ContractRuntime {
    async fn resolve(
        &self,
        target: &AgentRuntimeTarget,
    ) -> Result<AgentRuntimeConnection, AgentRuntimeError> {
        let store = self.store.get().unwrap().clone();
        let id = target.agent_id.clone();
        if target.runtime_generation != 1 || target.conversation_id != format!("conversation-{id}")
        {
            return Err(AgentRuntimeError::StaleGeneration);
        }
        if matches!(self.outcome, RuntimeOutcome::Reject)
            && store
                .get_agent(&id)
                .await
                .unwrap()
                .unwrap()
                .conversation
                .operation
                .as_ref()
                .is_some_and(|operation| operation.state == AgentOperationDelivery::DispatchFenced)
        {
            // Registry loss after fencing is pre-write; writer errors are already ambiguous.
            return Err(AgentRuntimeError::Unavailable(
                "contract runtime disappeared before write".into(),
            ));
        }
        let outcome = self.outcome.clone();
        let input = AgentRuntimeInputSender::spawn(4, move |_| {
            let store = store.clone();
            let id = id.clone();
            let outcome = outcome.clone();
            async move {
                let agent = store.get_agent(&id).await.unwrap().unwrap();
                assert!(agent
                    .conversation
                    .operation
                    .as_ref()
                    .is_some_and(
                        |operation| operation.state == AgentOperationDelivery::DispatchFenced
                    ));
                match outcome {
                    RuntimeOutcome::Ambiguous => Err(AgentRuntimeInputError::Unavailable(
                        "contract writer lost its receipt".into(),
                    )),
                    RuntimeOutcome::Reject => unreachable!("pre-write registry loss was returned"),
                    RuntimeOutcome::Stop(reason) => {
                        tokio::spawn(stopped::after_acknowledgement(store, id, reason));
                        Ok(())
                    }
                    RuntimeOutcome::Complete | RuntimeOutcome::Fail => {
                        let update = if matches!(outcome, RuntimeOutcome::Fail) {
                            AgentRuntimeUpdate::Failed {
                                message: "contract runtime turn failed".into(),
                                capability: None,
                            }
                        } else {
                            AgentRuntimeUpdate::TurnEnded(AgentRuntimePromptResponse::new(
                                AgentRuntimeTurnStopReason::EndTurn,
                            ))
                        };
                        DurableAgentUpdateRecorder::new(store)
                            .record_update(
                                &id,
                                1,
                                &mut AgentMessageAssembly::default(),
                                update,
                                layer2_core::now_rfc3339(),
                            )
                            .await
                            .map_err(|error| AgentRuntimeInputError::Rejected(error.to_string()))?;
                        Ok(())
                    }
                    RuntimeOutcome::Accept => Ok(()),
                }
            }
        });
        Ok(AgentRuntimeConnection {
            capabilities: RuntimeCapabilities::new([
                RuntimeCapability::Conversation,
                RuntimeCapability::Snapshot,
                RuntimeCapability::Steer,
                RuntimeCapability::Interrupt,
                RuntimeCapability::Usage,
                RuntimeCapability::Resources,
            ]),
            input,
        })
    }

    async fn fence(&self, _: &AgentId, _: u64) -> Result<(), AgentRuntimeError> {
        Ok(())
    }
}

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    if matches!(
        name,
        "terminal_owner" | "terminal_anonymous" | "terminal_forbidden"
    ) {
        let (state, store) = agents::conversation_fixture().await;
        agents::insert_agent(
            &store,
            agents::seeded_agent("agt_contract", AgentStatus::Ready, 2, None),
            vec![],
        )
        .await;
        return Some(Scenario {
            router: crate::pty::router(state.reads),
            subject: (name != "terminal_anonymous").then_some("owner-a"),
            authorization: Authorization::new(if name == "terminal_forbidden" {
                vec![]
            } else {
                vec![Capability::UseOwnSandboxes]
            }),
            symbols: Default::default(),
        });
    }
    let (status, ambiguous) = match name {
        "agents_operating" | "agents_checkpoint" => (AgentStatus::Ready, false),
        "agents_approval" => (AgentStatus::WaitingApproval, false),
        "agents_paused" => (AgentStatus::Paused, false),
        "agents_ambiguous" => (AgentStatus::Ready, true),
        _ => return None,
    };
    let registry = Arc::new(ContractRuntime::new(ambiguous));
    let fixture = agents::fixture_with_registry(registry.clone()).await;
    registry.attach_store(fixture.store.clone());
    let logs = if status == AgentStatus::WaitingApproval {
        vec![
            AgentLogKind::TurnStarted {
                model: None,
                turn_sequence: 3,
                origin_key: None,
            },
            AgentLogKind::OwnerMessage {
                content: "Review this change".into(),
            },
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
        ]
    } else {
        vec![]
    };
    let agent = agents::seeded_agent(
        "agt_contract",
        status,
        2 + logs.len() as u64,
        (status == AgentStatus::WaitingApproval).then_some(3),
    );
    let agent = agents::insert_agent(&fixture.store, agent, logs).await;
    if name == "agents_checkpoint" {
        super::experience_cases::begin_checkpoint(&fixture.store, &agent.sandbox_id).await;
    }
    Some(Scenario {
        router: fixture.router,
        subject: Some("owner-a"),
        authorization: Authorization::new([Capability::UseOwnSandboxes]),
        symbols: Default::default(),
    })
}
