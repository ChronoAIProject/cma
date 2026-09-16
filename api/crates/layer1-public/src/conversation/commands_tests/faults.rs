use super::{execution::*, fixtures::*};
use axum::http::StatusCode;
use futures_util::StreamExt;
use layer2_core::{
    sandbox::{agent_io::*, store::*, AgentOperationDelivery, AgentOperationError},
    AgentId,
};
use serde_json::json;
use std::{sync::atomic::Ordering, time::Duration};
use tower::ServiceExt;

#[tokio::test]
async fn ambiguous_runtime_reply_retains_exact_binding_and_keyed_recovery_without_a_resend() {
    for path in [AGUI, RESPONSES] {
        let f = fixture(false).await;
        f.runtime.1.store(true, Ordering::SeqCst);
        let input = if path == AGUI {
            json!({"threadId":ID,"messages":[{"role":"user","content":"Retained"}]})
        } else {
            json!({"conversation":ID,"input":"Retained","stream":true})
        };
        let task = tokio::spawn(f.router.clone().oneshot(request(
            path,
            Some("lost-reply"),
            input.clone(),
        )));
        let admitted = admitted(&f).await.remove(0);
        let claimed = claim(&f).await;
        let operation = match f
            .state
            .operations
            .execute_claimed_command(&context("owner-a").caller, &claimed)
            .await
        {
            Err(AgentOperationError::DeliveryAmbiguous {
                operation_sequence, ..
            }) => operation_sequence,
            other => panic!("expected ambiguous delivery, got {other:?}"),
        };
        let retained = f
            .store
            .reconcile_command(&admitted.receipt.envelope.command_id)
            .await
            .unwrap();
        assert_eq!(retained.status, AgentResponseStatus::InProgress);
        assert_eq!(
            retained.work.dispatch.as_ref().unwrap().operation_sequence,
            operation
        );
        assert_eq!(
            retained.work.observation.as_ref().unwrap().delivery,
            AgentOperationDelivery::RecoveryRequired
        );
        assert!(retained.work.failure.is_none());
        let response = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()["x-cma-command-id"],
            admitted.receipt.envelope.command_id.as_str()
        );
        assert_eq!(
            response.headers()["x-cma-response-id"],
            admitted.receipt.response_id.as_ref().unwrap().as_str()
        );
        let mut stream = response.into_body().into_data_stream();
        let first = tokio::time::timeout(Duration::from_secs(2), stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let wire = std::str::from_utf8(&first).unwrap();
        assert!(wire.contains(if path == AGUI {
            "RUN_STARTED"
        } else {
            "response.created"
        }));
        assert!(!wire.contains("response.failed") && !wire.contains("RUN_ERROR"));
        drop(stream);
        let recovered = f
            .router
            .clone()
            .oneshot(request(path, Some("lost-reply"), input.clone()))
            .await
            .unwrap();
        assert_eq!(recovered.status(), StatusCode::OK);
        assert_eq!(
            recovered.headers()["x-cma-command-id"],
            admitted.receipt.envelope.command_id.as_str()
        );
        drop(recovered);
        assert_eq!(f.runtime.0.lock().unwrap().len(), 1);
        assert!(f
            .store
            .claim_next_command(CommandClaimRequest {
                agent_id: AgentId(ID.into()),
                holder: "replacement".into(),
                now: layer2_core::now_rfc3339(),
                expires_at: layer2_core::rfc3339_at(
                    std::time::SystemTime::now() + Duration::from_secs(30)
                ),
            })
            .await
            .unwrap()
            .is_none());
        finish(&f, "Runtime evidence arrived").await;
        let completed = f
            .store
            .reconcile_command(&admitted.receipt.envelope.command_id)
            .await
            .unwrap();
        assert_eq!(completed.status, AgentResponseStatus::Completed);
        assert_eq!(completed.work.dispatch, retained.work.dispatch);
        let replay = f
            .router
            .clone()
            .oneshot(request(path, Some("lost-reply"), input))
            .await
            .unwrap();
        assert!(body(replay).await.contains("Runtime evidence arrived"));
        assert_eq!(f.runtime.0.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn queued_cancellation_returns_original_receipt_without_creating_a_turn() {
    for path in [AGUI, RESPONSES] {
        let f = fixture(false).await;
        let input = if path == AGUI {
            json!({"threadId":ID,"messages":[{"role":"user","content":"Cancel before send"}]})
        } else {
            json!({"conversation":ID,"input":"Cancel before send","stream":true})
        };
        let task = tokio::spawn(f.router.clone().oneshot(request(
            path,
            Some("queued-cancel"),
            input,
        )));
        let admitted = admitted(&f).await.remove(0);
        f.state
            .commands
            .as_ref()
            .unwrap()
            .cancel_queued_response(
                &context("owner-a"),
                "fixture-credential",
                CommandSubmissionSource::Api,
                QueuedCancellationSubmission {
                    agent_id: AgentId(ID.into()),
                    idempotency_key: "cancel-source".into(),
                    target: QueuedCancellationTarget {
                        response_id: admitted.receipt.response_id.clone().unwrap(),
                        source_command_id: admitted.receipt.envelope.command_id.clone(),
                        expected_command_revision: admitted.revision,
                    },
                },
            )
            .await
            .unwrap();
        let response = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            response.headers()["x-cma-command-id"],
            admitted.receipt.envelope.command_id.as_str()
        );
        assert_eq!(
            response.headers()["x-cma-response-id"],
            admitted.receipt.response_id.as_ref().unwrap().as_str()
        );
        let wire = body(response).await;
        assert!(wire.contains("agent_command_cancelled"));
        assert!(!wire.contains("response.failed") && !wire.contains("RUN_STARTED"));
        let current = f
            .store
            .get_command(&admitted.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(current.status, AgentResponseStatus::Cancelled);
        assert!(current.work.dispatch.is_none());
        assert_eq!(
            f.store
                .get_agent(&AgentId(ID.into()))
                .await
                .unwrap()
                .unwrap()
                .conversation
                .last_seq,
            2
        );
        assert!(f.runtime.0.lock().unwrap().is_empty());
    }
}
