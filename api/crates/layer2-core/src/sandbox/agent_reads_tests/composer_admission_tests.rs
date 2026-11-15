//! A retained browser observation never grants runtime input authority (#1801).
use super::*;
use crate::sandbox::store::{
    SandboxLifecycleAgentScope, SandboxLifecycleExecutionStore, SandboxLifecycleMutation,
};

async fn change_parent(store: &InMemorySandboxStore, agent: &Agent, state: &str) {
    let mut parent = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
    let revision = parent.revision;
    parent.runtime = crate::sandbox::lifecycle_test_support::sandbox().runtime;
    parent.runtime.as_mut().unwrap().provision_generation = Some(1);
    parent.lifecycle.runtime_generation_high_water = 1;
    parent.revision += 1;
    store.save_sandbox(revision, parent.clone()).await.unwrap();
    let at = crate::now_rfc3339();
    let mut next = parent.clone();
    match state {
        "checkpoint" => next.begin_checkpoint_capture(at.clone()).unwrap(),
        "lifecycle" => next.begin_pause(at.clone(), None).unwrap(),
        "failed" => {
            next.status = SandboxStatus::Failed;
            next.failure = Some(TypedFailure {
                class: "fixture_failure".into(),
                detail: "Runtime unavailable".into(),
            });
            next.revision += 1;
        }
        "retired" => {
            next.status = SandboxStatus::Retired;
            next.retired_at = Some(at.clone());
            next.revision += 1;
        }
        _ => unreachable!(),
    }
    next.updated_at = at.clone();
    next.validate().unwrap();
    if state != "checkpoint" {
        store.save_sandbox(parent.revision, next).await.unwrap();
        return;
    }
    store
        .commit_sandbox_lifecycle(SandboxLifecycleMutation {
            guard: SandboxMutationGuard::capture(&parent),
            sandbox: next,
            agents: vec![],
            agent_scope: SandboxLifecycleAgentScope::Touched,
            appended_at: at,
            events: vec![SandboxEventKind::WorkStepCompleted {
                step: SandboxWorkStep::RunLifecycleOperation,
            }],
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn stale_composer_observations_cannot_dispatch_legacy_or_v2_input() {
    for state in ["checkpoint", "lifecycle", "failed", "retired"] {
        for v2 in [false, true] {
            let (store, access, _, agent) = execution_fixture(MAX_MEMBERSHIP_AGE).await;
            // Admit before the parent changes: even an accepted v2 receipt
            // cannot authorize delivery into a runtime being replaced.
            let held = if v2 {
                let command = claim(&store, &agent, "owner").await;
                let authorization = access
                    .authorize_personal_command_execution(
                        &context("owner").caller,
                        &command.receipt.envelope,
                    )
                    .await
                    .unwrap();
                Some((command, authorization))
            } else {
                None
            };
            change_parent(&store, &agent, state).await;
            let (runtime, sends) = runtime(store.clone(), None, 0);
            let operations = AgentOperationService::new(store.clone(), runtime);
            if let Some((command, authorization)) = held {
                let result = operations
                    .execute_resource_claim(&authorization, &command)
                    .await;
                match state {
                    "checkpoint" => assert!(matches!(
                        result,
                        Err(AgentOperationError::CheckpointInProgress)
                    )),
                    _ => assert!(matches!(
                        result,
                        Err(AgentOperationError::StatusConflict { .. })
                    )),
                }
                let retained = store
                    .get_command(&command.receipt.envelope.command_id)
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(retained.receipt, command.receipt);
                assert!(retained.work.dispatch.is_none());
            } else {
                let result = operations
                    .send_message(
                        &context("owner").caller,
                        &agent.id,
                        "Retain until admissible".into(),
                    )
                    .await;
                match state {
                    "checkpoint" => assert!(matches!(
                        result,
                        Err(AgentOperationError::CheckpointInProgress)
                    )),
                    "lifecycle" => {
                        assert!(matches!(result, Ok(AgentMessageOutcome::Queued { .. })))
                    }
                    _ => assert!(matches!(
                        result,
                        Err(AgentOperationError::StatusConflict { .. })
                    )),
                }
            }
            assert_eq!(sends.load(Ordering::SeqCst), 0, "{state}, v2={v2}");
        }
    }
}
