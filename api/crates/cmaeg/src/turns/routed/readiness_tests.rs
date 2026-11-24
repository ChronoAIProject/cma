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
                // Prepared also exists before the first POST. Observe that POST
                // before waiting for the refusal to restore its frozen state.
                if app
                    .provider
                    .seen()
                    .iter()
                    .any(|request| request.method == "POST")
                    && app
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
            matches!(result, Ok(DispatchOutcome::Terminal(Phase::Refused { code })) if code == "event_expired_before_start")
        } else {
            matches!(result, Err(ExecutionError::AuthorityChanged))
        });
        assert_eq!(http.seen().len(), 1);
        assert_eq!(setup.port.prepares.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn sender_context_refusals_are_strict_and_expire_without_another_post() {
    let cases: Vec<Value> = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/provider_sender_refusals.json"
    )))
    .unwrap();
    assert_eq!(cases.len(), 3);
    for case in cases {
        let status = case["status"].as_u64().unwrap() as u16;
        let code = case["body"]["error"]["code"].as_str().unwrap();
        for fault in [
            "none",
            "code",
            "status",
            "content_type",
            "header",
            "type",
            "malformed",
            "top_level",
        ] {
            let setup = Setup::new(supported()).await;
            let http = HttpServer::start().await;
            {
                let mut memory = http.memory.lock().unwrap();
                let config = &mut memory.config;
                config.post_status = if fault == "status" {
                    if status == 403 {
                        422
                    } else {
                        403
                    }
                } else {
                    status
                };
                config.post_error_content_type = case["content_type"].as_str().unwrap().into();
                config.post_error_retry_after = case["retry_after"].as_str().map(str::to_owned);
                let mut body = case["body"].clone();
                match fault {
                    "code" => body["error"]["code"] = "unrelated_failure".into(),
                    "content_type" => config.post_error_content_type = "text/plain".into(),
                    "header" => config.post_error_response_id = true,
                    "type" => body["error"]["type"] = "other_error".into(),
                    "top_level" => body = json!({"code":code}),
                    _ => {}
                }
                config.post_error_body = Some(if fault == "malformed" {
                    "{".into()
                } else {
                    body.to_string()
                });
            }
            let activity_server =
                crate::ingress::routing_store::test_server::Server::start_realtime().await;
            let activity = Arc::new(crate::activity::ActivityStore::new(
                activity_server.records.clone(),
                Arc::new(fixture::keys()),
            ));
            activity
                .received_at(
                    &setup.plan.identity.owner,
                    setup.plan.identity.record_id.as_str(),
                    &setup.plan.activity,
                    time::OffsetDateTime::now_utc(),
                )
                .await
                .unwrap();
            let dispatcher = Arc::new(HttpExecutionPort::new(
                http.client(true, Duration::from_millis(250)),
                setup.port.clone(),
                setup.server.store(),
                setup.journal().clone().with_activity(activity.clone()),
                setup.current.clone(),
            ))
            .dispatcher();
            let result = dispatcher
                .dispatch(&setup.plan.identity, &setup.target().id, "first")
                .await
                .unwrap();
            if fault != "none" {
                assert!(
                    matches!(result, DispatchOutcome::Unknown),
                    "{status} {fault}"
                );
                assert_eq!(stored(&setup).await.phase, Phase::Unknown);
            } else if status == 503 {
                assert!(matches!(result, DispatchOutcome::Retry));
                setup
                    .server
                    .memory
                    .clock
                    .store(setup.target().launch.event.expires_at, Ordering::SeqCst);
                assert!(
                    matches!(dispatcher.dispatch(&setup.plan.identity,&setup.target().id,"expired").await.unwrap(),
                    DispatchOutcome::Terminal(Phase::Refused { code }) if code == "event_expired_before_start")
                );
            } else {
                assert!(
                    matches!(result,DispatchOutcome::Terminal(Phase::Refused { code: observed }) if observed == code)
                );
                assert!(matches!(
                    dispatcher
                        .dispatch(&setup.plan.identity, &setup.target().id, "again")
                        .await
                        .unwrap(),
                    DispatchOutcome::Terminal(_)
                ));
            }
            assert_eq!(http.seen().len(), 1, "no transport boundary retries");
            if fault == "none" {
                let page = activity
                    .latest(
                        &setup.plan.identity.owner,
                        setup.plan.identity.record_id.as_str(),
                    )
                    .await
                    .unwrap();
                let consumer = &page.items[0].consumers[0];
                assert_eq!(
                    consumer.refusal_code.as_deref(),
                    Some(if status == 503 {
                        "event_expired_before_start"
                    } else {
                        code
                    })
                );
                assert_eq!(consumer.state, crate::activity::ConsumerState::Refused);
                assert!(consumer.completed_at.is_some());
                assert!(consumer.deliveries.is_empty());
            }
        }
    }
}

#[tokio::test]
async fn sender_context_outage_retries_after_cooldown_and_admits_once() {
    let mut app = App::new(true).await;
    {
        let mut memory = app.provider.memory.lock().unwrap();
        memory.config.post_status = 503;
        memory.config.post_error_body = Some(json!({"error":{"type":"server_error","code":"verified_sender_context_unavailable","message":"no admission","param":"event_context"}}).to_string());
    }
    let event = app.event("sender-outage").to_string();
    let response = app.request(event.clone(), true).send().await.unwrap();
    tokio::time::timeout(Duration::from_secs(4), async {
        while !app.provider.seen().iter().any(|r| r.method == "POST") {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        app.provider
            .seen()
            .iter()
            .filter(|r| r.method == "POST")
            .count(),
        1
    );
    app.provider.memory.lock().unwrap().config.post_status = 200;
    let stream = tokio::time::timeout(Duration::from_secs(10), response.text())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stream.matches("event: cmaeg.completed").count(), 1);
    app.drain().await;
    let posts: Vec<_> = app
        .provider
        .seen()
        .into_iter()
        .filter(|r| r.method == "POST")
        .collect();
    assert_eq!(posts.len(), 2);
    assert_eq!(posts[0].body, posts[1].body);
    assert_eq!(posts[0].idempotency, posts[1].idempotency);
    assert_eq!(app.request(event, true).send().await.unwrap().status(), 202);
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

#[tokio::test]
async fn nyxid_pre_admission_validation_refusal_is_terminal_and_never_reposted() {
    // NyxID refuses an unsupported notification (for example a Discord
    // deletion on an older replica) with this body before accepting it.
    let refusal = json!({"error":"validation_error","error_code":1008,
        "message":"Request validation failed"});
    for fault in [
        "none",
        "error",
        "error_code",
        "message",
        "header",
        "content_type",
        "malformed",
        "status",
    ] {
        let setup = Setup::new(supported()).await;
        let http = HttpServer::start().await;
        {
            let mut memory = http.memory.lock().unwrap();
            let config = &mut memory.config;
            config.post_status = if fault == "status" { 409 } else { 400 };
            config.post_error_content_type = if fault == "content_type" {
                "text/plain"
            } else {
                "application/json"
            }
            .into();
            config.post_error_response_id = fault == "header";
            let mut body = refusal.clone();
            match fault {
                "error" => body["error"] = "bad_request".into(),
                "error_code" => body["error_code"] = 1009.into(),
                "message" => {
                    body.as_object_mut().unwrap().remove("message");
                }
                _ => {}
            }
            config.post_error_body = Some(if fault == "malformed" {
                "{".into()
            } else {
                body.to_string()
            });
        }
        let dispatcher = port(&setup, &http).dispatcher();
        let result = dispatcher
            .dispatch(&setup.plan.identity, &setup.target().id, "first")
            .await
            .unwrap();
        if fault == "none" {
            assert!(
                matches!(&result, DispatchOutcome::Terminal(Phase::Refused { code })
                    if code == "provider_validation_refused"),
                "a documented pre-admission refusal releases the queue head"
            );
            assert!(matches!(
                dispatcher
                    .dispatch(&setup.plan.identity, &setup.target().id, "again")
                    .await
                    .unwrap(),
                DispatchOutcome::Terminal(_)
            ));
        } else {
            assert!(matches!(result, DispatchOutcome::Unknown), "{fault}");
            assert_eq!(stored(&setup).await.phase, Phase::Unknown, "{fault}");
        }
        assert_eq!(http.seen().len(), 1, "no transport boundary retries");
    }
}
