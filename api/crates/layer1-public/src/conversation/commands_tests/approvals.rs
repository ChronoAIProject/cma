use super::{execution::*, fixtures::*};
use axum::http::StatusCode;
use layer2_core::{
    sandbox::{agent_io::*, store::AgentStore, *},
    AgentId,
};
use serde_json::{json, Value};
use std::time::Duration;
use tower::ServiceExt;

async fn pending(f: &Fixture, id: &str) {
    let mut agent = f
        .store
        .get_agent(&AgentId(ID.into()))
        .await
        .unwrap()
        .unwrap();
    let at = layer2_core::now_rfc3339();
    let mut logs = vec![];
    if let Some(operation) = agent.conversation.operation.as_mut() {
        if operation.state != AgentOperationDelivery::ExecutionObserved {
            operation.mark_execution_observed(at.clone()).unwrap();
            logs.push(AgentLogKind::OperationDeliveryChanged {
                operation_sequence: operation.sequence,
                operation: operation.intent.kind(),
                state: operation.state,
            });
        }
    }
    logs.push(AgentLogKind::ApprovalRequested {
        approval_id: id.into(),
        action: Some("Review".into()),
        detail: None,
        runtime_locator: None,
    });
    if agent.status != AgentStatus::WaitingApproval {
        logs.push(AgentLogKind::StatusChanged {
            from: agent.status,
            to: AgentStatus::WaitingApproval,
        });
        agent
            .transition(AgentStatus::WaitingApproval, at, None)
            .unwrap();
    }
    commit(f, agent, logs).await;
}

#[tokio::test]
async fn approval_mapping_and_resume_precedence_preserve_the_existing_command_contract() {
    for (status, decision, expected) in [
        (
            Some("resolved"),
            Some("allow"),
            AgentApprovalDecision::Allow,
        ),
        (Some("resolved"), Some("deny"), AgentApprovalDecision::Deny),
        (
            Some("resolved"),
            Some("abort"),
            AgentApprovalDecision::Abort,
        ),
        (
            Some("resolved"),
            Some("unknown"),
            AgentApprovalDecision::Deny,
        ),
        (Some("pending"), Some("allow"), AgentApprovalDecision::Deny),
        (None, None, AgentApprovalDecision::Deny),
    ] {
        let f = fixture(true).await;
        pending(&f, "approval-a").await;
        let input = json!({"threadId":ID,"messages":[{"role":"user","content":"Ignored"}],
            "resume":[{"interruptId":"approval-a","status":status,"payload":{"decision":decision}}]});
        let task = tokio::spawn(
            f.router
                .clone()
                .oneshot(request(AGUI, Some("answer"), input)),
        );
        let record = admitted(&f).await.remove(0);
        task.abort();
        assert_eq!(
            record.receipt.envelope.command,
            AgentCommand::AnswerApproval {
                approval_id: "approval-a".into(),
                decision: expected
            }
        );
        assert_eq!(record.receipt.response_id, None);
        assert!(f.runtime.0.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn approval_stream_begins_at_its_dispatch_and_keyed_replay_reuses_the_original_turn() {
    let f = fixture(true).await;
    pending(&f, "approval-a").await;
    let input = json!({"threadId":ID,"resume":[{"interruptId":"approval-a","status":"resolved","payload":{"decision":"allow"}}]});
    let task = tokio::spawn(
        f.router
            .clone()
            .oneshot(request(AGUI, Some("answer"), input.clone())),
    );
    let admitted = admitted(&f).await.remove(0);
    pending(&f, "unrelated").await;
    let agent = f
        .store
        .get_agent(&AgentId(ID.into()))
        .await
        .unwrap()
        .unwrap();
    commit(
        &f,
        agent,
        vec![AgentLogKind::ApprovalResolved {
            approval_id: "unrelated".into(),
            decision: "deny".into(),
        }],
    )
    .await;
    let dispatched = execute(&f).await;
    let binding = dispatched.work.dispatch.unwrap();
    assert!(binding.operation_sequence > admitted.receipt.event_cursor);
    finish(&f, "Approval output").await;
    let response = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response.headers().contains_key("x-cma-response-id"));
    let wire = body(response).await;
    assert!(!wire.contains("unrelated"));
    let frames = events(&wire);
    assert_eq!(frames[0]["type"], "RUN_STARTED");
    assert!(frames
        .iter()
        .all(|event| event["runId"].as_str() == Some(binding.turn_sequence.to_string().as_str())));
    assert!(wire.contains("Approval output"));
    let response = f
        .router
        .clone()
        .oneshot(request(AGUI, Some("answer"), input.clone()))
        .await
        .unwrap();
    assert_eq!(body(response).await, wire);
    assert_eq!(f.runtime.0.lock().unwrap().len(), 1);
    let mut changed = input;
    changed["resume"][0]["payload"]["decision"] = Value::String("deny".into());
    assert_eq!(
        f.router
            .clone()
            .oneshot(request(AGUI, Some("answer"), changed))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
}
