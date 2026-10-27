use super::{
    adapter_test_server::{Config, HttpServer},
    custom_http_fixture::App,
    delivery_tests::stored,
    test_support::*,
    *,
};
use serde_json::{json, Value};
use std::{sync::atomic::Ordering, sync::Arc, time::Duration};

fn readiness(config: &mut Config) {
    config.post_status = 429;
    config.post_error_body = Some(
        json!({"error":{"type":"invalid_request_error","code":"conversation_not_ready",
            "message":"The binding conversation is not ready.","param":"conversation"}})
        .to_string(),
    );
}

fn port(setup: &Setup, http: &HttpServer) -> Arc<HttpExecutionPort> {
    Arc::new(HttpExecutionPort::new(
        http.client(true, Duration::from_millis(250)),
        setup.port.clone(),
        setup.server.store(),
        setup.journal().clone(),
        setup.current.clone(),
    ))
}

#[tokio::test]
async fn cold_custom_conversation_retries_frozen_turn_then_completes_once() {
    let mut app = App::new(true).await;
    readiness(&mut app.provider.memory.lock().unwrap().config);
    let event = app.event("cold-conversation").to_string();
    let response = app
        .request(event.clone(), true)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let plan = tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            if let Some(plan) = app.plans().await.first() {
                if app
                    .journal
                    .read::<TargetState>(&plan.targets[0].id)
                    .await
                    .unwrap()
                    .is_some_and(|(state, _)| state.phase == Phase::Prepared)
                {
                    break plan.clone();
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("readiness refusal must retain a retryable frozen turn");
    let first = app.provider.seen();
    assert_eq!(
        first
            .iter()
            .map(|request| request.method.as_str())
            .collect::<Vec<_>>(),
        ["PUT", "POST"]
    );
    assert_eq!(app.provider.memory.lock().unwrap().conversations.len(), 1);
    let activity = app
        .activity
        .latest(&plan.identity.owner, plan.identity.record_id.as_str())
        .await
        .unwrap();
    assert!(activity.items[0].consumers[0].response_id.is_none());
    assert_ne!(
        activity.items[0].consumers[0].state,
        crate::activity::ConsumerState::Unknown
    );

    // The provider becomes ready after its first refusal. Only the scheduler's
    // existing five-second cooldown may issue the same frozen POST again.
    app.provider.memory.lock().unwrap().config.post_status = 200;
    let stream = response.text().await.unwrap();
    assert!(
        stream.contains("hello ") && stream.contains("world"),
        "{stream}"
    );
    assert_eq!(stream.matches("event: cmaeg.completed").count(), 1);
    app.drain().await;
    let state = app
        .journal
        .read::<TargetState>(&plan.targets[0].id)
        .await
        .unwrap()
        .unwrap()
        .0;
    assert!(matches!(state.phase, Phase::Completed { .. }));
    let activity = app
        .activity
        .latest(&plan.identity.owner, plan.identity.record_id.as_str())
        .await
        .unwrap();
    assert_eq!(activity.items.len(), 1);
    assert_eq!(
        activity.items[0].consumers[0].state,
        crate::activity::ConsumerState::Completed
    );
    assert_eq!(
        activity.items[0].consumers[0].response_id.as_deref(),
        Some("resp_accepted")
    );
    let duplicate = app.request(event, true).send().await.unwrap();
    assert_eq!(duplicate.status(), 202);
    let seen = app.provider.seen();
    let posts: Vec<_> = seen
        .iter()
        .filter(|request| request.method == "POST")
        .collect();
    assert_eq!(
        posts.len(),
        2,
        "one readiness refusal and one accepted turn"
    );
    assert_eq!(posts[0].body, posts[1].body);
    assert_eq!(posts[0].idempotency, posts[1].idempotency);
    assert!(posts[0].idempotency.is_some());
    assert_eq!(
        seen.iter()
            .filter(|request| request.method == "PUT")
            .count(),
        1
    );
    assert_eq!(app.plans().await.len(), 1);
    assert!(app.waiters.is_empty());
}

#[tokio::test]
async fn readiness_retry_rechecks_original_expiry_and_current_authority() {
    for expired in [true, false] {
        let setup = Setup::new(supported()).await;
        let http = HttpServer::start().await;
        readiness(&mut http.memory.lock().unwrap().config);
        let dispatcher = port(&setup, &http).dispatcher();
        assert!(matches!(
            dispatcher
                .dispatch(&setup.plan.identity, &setup.target().id, "first")
                .await
                .unwrap(),
            DispatchOutcome::Retry
        ));
        assert_eq!(stored(&setup).await.phase, Phase::Prepared);
        http.memory.lock().unwrap().config.post_status = 200;
        if expired {
            setup
                .server
                .memory
                .clock
                .store(setup.target().launch.event.expires_at, Ordering::SeqCst);
        } else {
            setup.current.revoke_at.store(0, Ordering::SeqCst);
        }
        let result = dispatcher
            .dispatch(&setup.plan.identity, &setup.target().id, "retry")
            .await;
        assert!(if expired {
            matches!(result, Err(ExecutionError::Expired))
        } else {
            matches!(result, Err(ExecutionError::AuthorityChanged))
        });
        assert_eq!(http.seen().len(), 1);
        assert_eq!(setup.port.prepares.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn incomplete_or_contradictory_readiness_is_unknown_and_never_reposted() {
    for fault in [
        "generic",
        "admission_conflict",
        "queue_full",
        "status",
        "content_type",
        "header",
        "response",
        "top_level",
        "type",
        "malformed",
        "oversized",
        "idle",
        "transport",
    ] {
        let setup = Setup::new(supported()).await;
        let http = HttpServer::start().await;
        {
            let mut memory = http.memory.lock().unwrap();
            let config = &mut memory.config;
            readiness(config);
            let mut body: Value =
                serde_json::from_str(config.post_error_body.as_ref().unwrap()).unwrap();
            match fault {
                "generic" => body["error"]["code"] = json!("response_unavailable"),
                "admission_conflict" => {
                    body["error"]["code"] = json!("agent_command_admission_conflict")
                }
                "queue_full" => body["error"]["code"] = json!("agent_command_queue_full"),
                "status" => config.post_status = 503,
                "content_type" => config.post_error_content_type = "text/event-stream".into(),
                "header" => config.post_error_response_id = true,
                "response" => body["response"] = json!({"id":"resp_accepted","status":"queued"}),
                "top_level" => body = json!({"code":"conversation_not_ready"}),
                "type" => body["error"]["type"] = json!("server_error"),
                "oversized" => body["error"]["message"] = json!("x".repeat(64 * 1024)),
                "malformed" => {}
                "idle" => config.post_error_idle = true,
                "transport" => config.post_error_transport_failure = true,
                _ => unreachable!(),
            }
            config.post_error_body = Some(if fault == "malformed" {
                "{\"error\":".into()
            } else {
                body.to_string()
            });
        }
        let dispatcher = port(&setup, &http).dispatcher();
        for _ in 0..2 {
            assert!(
                matches!(
                    dispatcher
                        .dispatch(&setup.plan.identity, &setup.target().id, "worker")
                        .await
                        .unwrap(),
                    DispatchOutcome::Unknown
                ),
                "{fault}"
            );
        }
        assert_eq!(http.seen().len(), 1, "{fault}");
        assert_eq!(setup.port.prepares.load(Ordering::SeqCst), 1, "{fault}");
    }
}
