use layer3_adapter::{
    AdapterError, ConversationUpdate, PermissionRequest, PromptResponse, RequestPermissionRequest,
    ToolCallUpdate, ToolCallUpdateFields, TurnStopReason,
};

use super::command_work_tests::{admit, fixture, request};
use super::fixtures::*;
use crate::sandbox::agent_io::*;
use crate::sandbox::store::{AgentLogStore, AgentStore};
use crate::sandbox::*;

#[path = "credit_failure_tests.rs"]
mod credit_failure_tests;

#[path = "budget_recovery_tests.rs"]
mod budget_recovery_tests;

#[path = "certified_evidence_tests.rs"]
mod certified_evidence_tests;

async fn claimed(store: &InMemorySandboxStore, agent: &Agent) -> AdmittedAgentCommand {
    store
        .claim_next_command(request(agent, "worker", &crate::now_rfc3339()))
        .await
        .unwrap()
        .unwrap()
}

async fn end(store: std::sync::Arc<InMemorySandboxStore>, agent: &Agent, reason: TurnStopReason) {
    DurableAgentUpdateRecorder::new(store)
        .record_update(
            &agent.id,
            3,
            &mut AgentMessageAssembly::default(),
            ConversationUpdate::TurnEnded(PromptResponse::new(reason)),
            crate::now_rfc3339(),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn task_acceptance_survives_turn_completion_without_the_required_output() {
    use crate::sandbox::{answer_deliverables::DeliverableResource, task_acceptance::*};
    let (store, agent, parent) = fixture(AgentStatus::Ready).await;
    let mut command = AgentCommand::message("Prepare the report");
    let AgentCommand::Message { input, .. } = &mut command else {
        unreachable!()
    };
    input.task_acceptance = Some(TaskAcceptanceIntent {
        version: 1,
        action: TaskAcceptanceAction::Declare {
            requirements: vec![TaskRequirement {
                id: "report".into(),
                required: true,
                evidence: TaskRequirementEvidence::Deliverable {
                    resource: DeliverableResource::LiveFile {
                        path: "out/report.csv".into(),
                    },
                },
            }],
        },
    });
    let admitted = admit(&store, &agent, &parent, "task", command).await;
    let work = claimed(&store, &agent).await;
    let (sender, inputs) = recording_sender(Ok(()));
    let service = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    service
        .execute_claimed_command(&owner(), &work)
        .await
        .unwrap();
    end(store.clone(), &agent, TurnStopReason::EndTurn).await;
    let retained = store.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(retained.status, AgentStatus::Ready);
    let acceptance = retained.conversation.observations.task_acceptance.unwrap();
    assert_eq!(acceptance.task_id, admitted.receipt.envelope.command_id);
    assert_eq!(
        acceptance.verdict,
        TaskAcceptanceVerdict::NeedsAttention(TaskAcceptanceReason::RequiredDeliverablesMissing)
    );
    assert_eq!(
        store
            .reconcile_command(&acceptance.task_id)
            .await
            .unwrap()
            .status,
        AgentResponseStatus::Completed
    );
    assert_eq!(inputs.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_restart_reconstructs_delivery_then_turn_completion_without_changing_admission() {
    for reason in [TurnStopReason::EndTurn, TurnStopReason::Cancelled] {
        let (store, agent, parent) = fixture(AgentStatus::Ready).await;
        let admitted = admit(&store, &agent, &parent, "one", AgentCommand::message("one")).await;
        let work = claimed(&store, &agent).await;
        let (sender, inputs) = recording_sender(Ok(()));
        let service = AgentOperationService::new(
            store.clone(),
            TestRuntimeRegistry::new(full_capabilities(), sender),
        );
        service
            .execute_claimed_command(&owner(), &work)
            .await
            .unwrap();
        drop(service);
        let id = &admitted.receipt.envelope.command_id;
        let running = store.reconcile_command(id).await.unwrap();
        assert_eq!(running.status, AgentResponseStatus::InProgress);
        assert_eq!(
            running.work.observation.as_ref().unwrap().delivery,
            AgentOperationDelivery::WriteConfirmed
        );
        assert!(running.work.lease.is_none());
        assert_eq!(store.reconcile_command(id).await.unwrap(), running);
        let status = if reason == TurnStopReason::Cancelled {
            AgentResponseStatus::Cancelled
        } else {
            AgentResponseStatus::Completed
        };
        end(store.clone(), &agent, reason).await;
        let done = store.reconcile_command(id).await.unwrap();
        assert_eq!(done.status, status);
        assert_eq!(done.receipt, admitted.receipt);
        assert_eq!(done.work.dispatch, running.work.dispatch);
        assert_eq!(
            done.work.observation.as_ref().unwrap().delivery,
            AgentOperationDelivery::ExecutionObserved
        );
        assert_eq!(store.reconcile_command(id).await.unwrap(), done);
        assert_eq!(inputs.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn ambiguous_dispatch_stays_unresolved_until_runtime_evidence_arrives() {
    let (store, agent, parent) = fixture(AgentStatus::Ready).await;
    let admitted = admit(&store, &agent, &parent, "one", AgentCommand::message("one")).await;
    let work = claimed(&store, &agent).await;
    let (sender, inputs) = recording_sender(Err(AdapterError::Rejected("receipt lost".into())));
    let service = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    assert!(matches!(
        service.execute_claimed_command(&owner(), &work).await,
        Err(AgentOperationError::DeliveryAmbiguous { .. })
    ));
    let id = &admitted.receipt.envelope.command_id;
    let ambiguous = store.reconcile_command(id).await.unwrap();
    assert_eq!(ambiguous.status, AgentResponseStatus::InProgress);
    assert_eq!(
        ambiguous.work.observation.as_ref().unwrap().delivery,
        AgentOperationDelivery::RecoveryRequired
    );
    assert!(ambiguous.work.failure.is_none());
    assert!(store
        .claim_next_command(request(&agent, "replacement", &crate::now_rfc3339()))
        .await
        .unwrap()
        .is_none());
    end(store.clone(), &agent, TurnStopReason::EndTurn).await;
    assert_eq!(
        store.reconcile_command(id).await.unwrap().status,
        AgentResponseStatus::Completed
    );
    assert_eq!(inputs.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn pre_write_failure_is_terminal_and_never_returns_to_the_queue() {
    let (store, agent, parent) = fixture(AgentStatus::Ready).await;
    let admitted = admit(&store, &agent, &parent, "one", AgentCommand::message("one")).await;
    let work = claimed(&store, &agent).await;
    let (sender, inputs) = recording_sender(Ok(()));
    let service = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender).fail_on_resolution(2),
    );
    assert!(matches!(
        service.execute_claimed_command(&owner(), &work).await,
        Err(AgentOperationError::RuntimeUnavailable(_))
    ));
    let failed = store
        .reconcile_command(&admitted.receipt.envelope.command_id)
        .await
        .unwrap();
    assert_eq!(failed.status, AgentResponseStatus::Failed);
    assert_eq!(
        failed.work.failure.unwrap().code,
        CommandFailureCode::RuntimeUnavailable
    );
    assert_eq!(
        failed.work.observation.unwrap().delivery,
        AgentOperationDelivery::PreWriteFailed
    );
    assert!(failed.work.lease.is_none());
    assert!(store
        .claim_next_command(request(&agent, "replacement", &crate::now_rfc3339()))
        .await
        .unwrap()
        .is_none());
    assert!(inputs.lock().unwrap().is_empty());
}

#[tokio::test]
async fn approval_wait_and_control_completion_are_recovered_independently() {
    let (store, agent, parent) = fixture(AgentStatus::Ready).await;
    let admitted = admit(&store, &agent, &parent, "one", AgentCommand::message("one")).await;
    let work = claimed(&store, &agent).await;
    let (sender, _) = recording_sender(Ok(()));
    let service = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    service
        .execute_claimed_command(&owner(), &work)
        .await
        .unwrap();
    let id = &admitted.receipt.envelope.command_id;
    store.reconcile_command(id).await.unwrap();
    let recorder = DurableAgentUpdateRecorder::new(store.clone());
    for approval_id in ["approval-one", "approval-two"] {
        recorder
            .record_update(
                &agent.id,
                3,
                &mut AgentMessageAssembly::default(),
                ConversationUpdate::Asked(PermissionRequest {
                    id: approval_id.into(),
                    locator: None,
                    request: RequestPermissionRequest::new(
                        "conversation-operations",
                        ToolCallUpdate::new(
                            approval_id,
                            ToolCallUpdateFields::new().title("Run checks"),
                        ),
                        vec![],
                    ),
                }),
                crate::now_rfc3339(),
            )
            .await
            .unwrap();
    }
    assert_eq!(
        store.reconcile_command(id).await.unwrap().status,
        AgentResponseStatus::WaitingAction
    );
    for (index, approval_id) in ["approval-one", "approval-two"].into_iter().enumerate() {
        let waiting = store.get_agent(&agent.id).await.unwrap().unwrap();
        let answer = admit(
            &store,
            &waiting,
            &parent,
            approval_id,
            AgentCommand::AnswerApproval {
                approval_id: approval_id.into(),
                decision: AgentApprovalDecision::Allow,
            },
        )
        .await;
        let answer_claim = claimed(&store, &waiting).await;
        service
            .execute_claimed_command(&owner(), &answer_claim)
            .await
            .unwrap();
        let done = store
            .reconcile_command(&answer.receipt.envelope.command_id)
            .await
            .unwrap();
        assert_eq!(done.status, AgentResponseStatus::Completed);
        let expected = if index == 0 {
            AgentResponseStatus::WaitingAction
        } else {
            AgentResponseStatus::InProgress
        };
        assert_eq!(store.reconcile_command(id).await.unwrap().status, expected);
    }
    end(store.clone(), &agent, TurnStopReason::EndTurn).await;
    assert_eq!(
        store.reconcile_command(id).await.unwrap().status,
        AgentResponseStatus::Completed
    );
}

#[tokio::test]
async fn observing_an_older_command_does_not_release_a_newer_control_claim() {
    let (store, agent, parent) = fixture(AgentStatus::Ready).await;
    let admitted = admit(&store, &agent, &parent, "one", AgentCommand::message("one")).await;
    let work = claimed(&store, &agent).await;
    let (sender, _) = recording_sender(Ok(()));
    let service = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    service
        .execute_claimed_command(&owner(), &work)
        .await
        .unwrap();
    store
        .release_command_claim(&work.work.lease.unwrap().claim, crate::now_rfc3339())
        .await
        .unwrap();
    let working = store.get_agent(&agent.id).await.unwrap().unwrap();
    admit(
        &store,
        &working,
        &parent,
        "control",
        AgentCommand::Interrupt,
    )
    .await;
    let control = claimed(&store, &working).await;
    store
        .reconcile_command(&admitted.receipt.envelope.command_id)
        .await
        .unwrap();
    admit(
        &store,
        &working,
        &parent,
        "another",
        AgentCommand::Interrupt,
    )
    .await;
    assert!(store
        .claim_next_command(request(&agent, "intruder", &crate::now_rfc3339()))
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        store
            .get_command(&control.receipt.envelope.command_id)
            .await
            .unwrap(),
        Some(control)
    );
}

#[tokio::test]
async fn missing_or_wrong_command_evidence_cannot_publish_an_outcome() {
    let (store, agent, parent) = fixture(AgentStatus::Ready).await;
    let admitted = admit(&store, &agent, &parent, "one", AgentCommand::message("one")).await;
    let work = claimed(&store, &agent).await;
    let (sender, _) = recording_sender(Ok(()));
    let service = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    service
        .execute_claimed_command(&owner(), &work)
        .await
        .unwrap();
    let logs = store.agent_log(&agent.id, 0, 50).await.unwrap();
    let mut broken = logs.clone();
    broken.remove(1);
    store.replace_agent_log_for_test(&agent.id, broken);
    let id = &admitted.receipt.envelope.command_id;
    let before = store.get_command(id).await.unwrap();
    assert_eq!(
        store.reconcile_command(id).await,
        Err(CommandLedgerError::Corrupt)
    );
    assert_eq!(store.get_command(id).await.unwrap(), before);
    let mut broken = logs.clone();
    if let AgentLogKind::TurnStarted { origin_key, .. } = &mut broken[1].kind {
        *origin_key = Some("another-command".into());
    }
    store.replace_agent_log_for_test(&agent.id, broken);
    assert_eq!(
        store.reconcile_command(id).await,
        Err(CommandLedgerError::Corrupt)
    );
    store.replace_agent_log_for_test(&agent.id, logs);
    assert!(store.reconcile_command(id).await.is_ok());
}

#[tokio::test]
async fn runtime_retirement_preserves_uncertain_writes_when_it_interrupts_the_turn() {
    for ambiguous in [false, true] {
        let (store, agent, parent) = fixture(AgentStatus::Ready).await;
        let admitted = admit(&store, &agent, &parent, "one", AgentCommand::message("one")).await;
        let work = claimed(&store, &agent).await;
        let result = if ambiguous {
            Err(AdapterError::Rejected("receipt lost".into()))
        } else {
            Ok(())
        };
        let (sender, inputs) = recording_sender(result);
        let service = AgentOperationService::new(
            store.clone(),
            TestRuntimeRegistry::new(full_capabilities(), sender),
        );
        assert_eq!(
            service
                .execute_claimed_command(&owner(), &work)
                .await
                .is_err(),
            ambiguous
        );
        let current = store.get_agent(&agent.id).await.unwrap().unwrap();
        let now = crate::now_rfc3339();
        let (retired, kinds) =
            crate::sandbox::runtime_recovery_agents::recover_agent(&parent, &current, &now, false)
                .unwrap();
        let mut log = store.agent_log(&agent.id, 0, 50).await.unwrap();
        for kind in kinds {
            log.push(AgentLogEntry {
                agent_id: agent.id.clone(),
                seq: log.len() as u64 + 1,
                appended_at: now.clone(),
                kind,
            });
        }
        store.save_agent(current.revision, retired).await.unwrap();
        store.replace_agent_log_for_test(&agent.id, log);
        let outcome = store
            .reconcile_command(&admitted.receipt.envelope.command_id)
            .await
            .unwrap();
        if ambiguous {
            assert_eq!(outcome.status, AgentResponseStatus::Failed);
            assert_eq!(
                outcome.work.failure.unwrap().code,
                CommandFailureCode::RuntimeLost
            );
            assert_eq!(
                outcome.work.observation.unwrap().delivery,
                AgentOperationDelivery::RuntimeLost
            );
        } else {
            assert_eq!(outcome.status, AgentResponseStatus::Cancelled);
        }
        assert!(outcome.work.dispatch.is_some());
        assert!(outcome.work.lease.is_none());
        assert_eq!(inputs.lock().unwrap().len(), 1);
    }
}

#[path = "../agent_io/reconcile_pacing_tests.rs"]
mod reconcile_pacing;
