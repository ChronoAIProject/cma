use super::{
    adapter_test_server::HttpServer,
    delivery_tests::{command, lane, stored},
    scheduler::{Disposition, TargetExecutor, WorkItem},
    test_support::*,
    *,
};
use crate::{
    delivery::{
        durable::receipt_for,
        intents::{DeliveryCommand, DeliveryOutcome},
    },
    http::readiness::Readiness,
    turns::tracker::TurnTracker,
    DeliveryConfig,
};
use serde_json::{json, Value};
use std::{
    sync::{atomic::Ordering, Arc},
    time::Duration,
};
use tokio::sync::mpsc;
pub(crate) fn executor(
    setup: &Setup,
    http: &HttpServer,
) -> (
    RoutedTargetExecutor,
    mpsc::Receiver<DeliveryCommand>,
    WorkItem,
) {
    executor_with_timeout(setup, http, Duration::from_secs(60))
}

pub(super) fn executor_with_timeout(
    setup: &Setup,
    http: &HttpServer,
    request_timeout: Duration,
) -> (
    RoutedTargetExecutor,
    mpsc::Receiver<DeliveryCommand>,
    WorkItem,
) {
    let port = Arc::new(HttpExecutionPort::new(
        http.client(true, request_timeout),
        setup.port.clone(),
        setup.server.store(),
        setup.journal().clone(),
        setup.current.clone(),
    ));
    let (delivery, receiver) = lane(setup);
    let executor = RoutedTargetExecutor::new(
        port,
        delivery,
        TurnTracker::new(Readiness::initialized()),
        &DeliveryConfig::default(),
    );
    (
        executor,
        receiver,
        WorkItem {
            identity: setup.plan.identity.clone(),
            target_id: setup.target().id.clone(),
        },
    )
}
fn lifecycle(sequence: u64, kind: &str, status: &str) -> Value {
    json!({"type":kind,"sequence_number":sequence,"response":{"id":"resp_accepted","object":"response","status":status,"conversation":{"id":"agt_native"},"metadata":{},"output":[]}})
}
pub(super) fn frames() -> Vec<Value> {
    vec![
        lifecycle(0, "response.created", "queued"),
        json!({"type":"response.output_text.delta","sequence_number":1,"item_id":"m","delta":"hello "}),
        json!({"type":"response.output_text.delta","sequence_number":2,"item_id":"m","delta":"world"}),
        lifecycle(3, "response.completed", "completed"),
    ]
}
#[tokio::test]
async fn executor_restarts_mid_text_with_get_only_and_delivers_complete_assembled_text() {
    let setup = Setup::new(supported()).await;
    let http = HttpServer::start().await;
    http.memory.lock().unwrap().config.replay_frames = Some(frames());
    let (mut first, mut rx, work) = executor(&setup, &http);
    first.frame_limit = 2;
    assert!(matches!(first.execute(&work).await, Disposition::Retry(_)));
    assert!(matches!(
        stored(&setup).await.phase,
        Phase::Accepted {
            durable_cursor: Some(1),
            ..
        }
    ));
    assert!(rx.try_recv().is_err());
    drop(first);
    let (second, mut rx, work) = executor(&setup, &http);
    let (result, ()) = tokio::join!(second.execute(&work), async {
        let reply = command(&mut rx).await;
        assert_eq!(reply.message.text.as_deref(), Some("hello world"));
        reply
            .receipt
            .send(receipt_for(DeliveryOutcome::Confirmed {
                platform_message_id: Some("reply".into()),
            }))
            .unwrap();
    });
    assert!(matches!(result, Disposition::Settled));
    assert!(matches!(
        stored(&setup).await.phase,
        Phase::Completed {
            durable_cursor: Some(3),
            ..
        }
    ));
    assert_eq!(http.seen().iter().filter(|v| v.method == "POST").count(), 1);
    assert_eq!(setup.port.prepares.load(Ordering::SeqCst), 1);
    assert!(http
        .seen()
        .last()
        .unwrap()
        .uri
        .ends_with("?starting_after=1"));
    let count = http.seen().len();
    assert!(matches!(second.execute(&work).await, Disposition::Settled));
    assert_eq!(http.seen().len(), count);
}
#[tokio::test]
async fn observation_error_reconnects_without_post_or_advancing_bad_cursor() {
    let setup = Setup::new(supported()).await;
    let http = HttpServer::start().await;
    http.memory.lock().unwrap().config.replay_frames = Some(frames()[..2].to_vec());
    http.memory.lock().unwrap().config.replay_tail =
        "event: error\ndata: {\"type\":\"error\",\"sequence_number\":99}\n\n".into();
    let (runner, mut rx, work) = executor(&setup, &http);
    assert!(matches!(runner.execute(&work).await, Disposition::Retry(_)));
    assert!(matches!(
        stored(&setup).await.phase,
        Phase::Accepted {
            durable_cursor: Some(1),
            ..
        }
    ));
    assert_eq!(http.seen().iter().filter(|v| v.method == "POST").count(), 1);
    assert_eq!(
        http.seen()
            .iter()
            .filter(|v| v.uri.contains("/events"))
            .count(),
        3
    );
    assert!(rx.try_recv().is_err());
}
#[tokio::test]
async fn unknown_platform_receipt_blocks_later_execution_without_duplicate_send() {
    let setup = Setup::new(supported()).await;
    let http = HttpServer::start().await;
    http.memory.lock().unwrap().config.replay_frames = Some(frames());
    let (runner, mut rx, work) = executor(&setup, &http);
    let (result, ()) = tokio::join!(runner.execute(&work), async {
        let reply = command(&mut rx).await;
        reply
            .receipt
            .send(receipt_for(DeliveryOutcome::Unknown {
                reason: "connection_lost".into(),
            }))
            .unwrap();
    });
    assert!(matches!(result, Disposition::Settled));
    let state = stored(&setup).await;
    assert!(matches!(
        state.phase,
        Phase::Accepted {
            durable_cursor: Some(2),
            ..
        }
    ));
    assert!(matches!(
        state.delivery.unwrap().status,
        DeliveryStatus::Unknown
    ));
    let requests = http.seen().len();
    assert!(matches!(runner.execute(&work).await, Disposition::Settled));
    assert!(rx.try_recv().is_err());
    assert_eq!(http.seen().len(), requests);
}
#[tokio::test]
async fn terminal_acknowledged_before_crash_completes_without_reconnecting() {
    let setup = Setup::new(supported()).await;
    let http = HttpServer::start().await;
    let (runner, _rx, work) = executor(&setup, &http);
    assert!(matches!(runner.execute(&work).await, Disposition::Settled));
    setup.server.change(&setup.target().id, |state| {
        state["phase"]["state"] = json!("accepted")
    });
    let requests = http.seen().len();
    let (resumed, _rx, work) = executor(&setup, &http);
    assert!(matches!(resumed.execute(&work).await, Disposition::Settled));
    assert!(matches!(
        stored(&setup).await.phase,
        Phase::Completed { .. }
    ));
    assert_eq!(http.seen().len(), requests);
}
#[tokio::test]
async fn missing_projection_for_existing_cursor_fails_closed() {
    let setup = Setup::new(supported()).await;
    let http = HttpServer::start().await;
    let (mut runner, mut rx, work) = executor(&setup, &http);
    runner.frame_limit = 1;
    assert!(matches!(runner.execute(&work).await, Disposition::Retry(_)));
    setup.server.change(&setup.target().id, |state| {
        state.as_object_mut().unwrap().remove("projection");
    });
    let count = http.seen().len();
    assert!(matches!(runner.execute(&work).await, Disposition::Settled));
    assert_eq!(http.seen().len(), count);
    assert!(rx.try_recv().is_err());
}
#[tokio::test]
async fn pinned_native_agent_and_revoked_authority_prevent_future_delivery() {
    for revoke in [false, true] {
        let setup = Setup::new(supported()).await;
        let http = HttpServer::start().await;
        let (mut runner, mut rx, work) = executor(&setup, &http);
        runner.frame_limit = 1;
        assert!(matches!(runner.execute(&work).await, Disposition::Retry(_)));
        if revoke {
            setup.current.revoke_at.store(0, Ordering::SeqCst);
        } else {
            http.memory.lock().unwrap().config.native_agent = "agt_other".into();
        }
        runner.frame_limit = 4096;
        runner.execute(&work).await;
        assert!(matches!(
            stored(&setup).await.phase,
            Phase::Accepted {
                durable_cursor: Some(0),
                ..
            }
        ));
        assert!(rx.try_recv().is_err());
        assert_eq!(http.seen().iter().filter(|v| v.method == "POST").count(), 1);
    }
}
#[tokio::test]
async fn idle_stream_renews_lease_and_bounded_invocation_releases_ownership() {
    async fn wait_for_state(
        setup: &Setup,
        ready: impl Fn(&TargetState) -> bool,
        description: &str,
    ) -> TargetState {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some((state, _)) = setup
                    .journal()
                    .read::<TargetState>(&setup.target().id)
                    .await
                    .unwrap()
                {
                    if ready(&state) {
                        return state;
                    }
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {description}"))
    }

    let setup = Setup::new(supported()).await;
    let http = HttpServer::start().await;
    http.memory.lock().unwrap().config.replay_frames =
        Some(vec![lifecycle(0, "response.created", "queued")]);
    http.memory.lock().unwrap().config.replay_idle = true;
    let (mut runner, _rx, work) = executor(&setup, &http);
    runner.renew_interval = Duration::from_millis(20);
    // Allow real loopback record IO without racing the observer against release.
    // Each persisted-state wait is bounded below this fixture-only deadline.
    runner.invocation_bound = Duration::from_secs(5);
    let started = tokio::time::Instant::now();
    let (result, ()) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(runner.execute(&work), async {
            let state = wait_for_state(
                &setup,
                |state| {
                    matches!(
                        state.phase,
                        Phase::Accepted {
                            durable_cursor: Some(0),
                            ..
                        }
                    )
                },
                "the idle stream's durable cursor",
            )
            .await;
            assert_eq!(state.ownership.until, fixture::NOW + 30);
            setup.server.memory.clock.fetch_add(20, Ordering::SeqCst);
            // No further frames arrive. This extension must come from the
            // executor's idle renewal, after its initial frame was committed.
            let state = wait_for_state(
                &setup,
                |state| state.ownership.until > fixture::NOW + 30,
                "the idle lease renewal before invocation release",
            )
            .await;
            assert!(state.ownership.until > fixture::NOW + 30);
        })
    })
    .await
    .expect("idle execution must release ownership within its bounded invocation");
    assert!(matches!(result, Disposition::Retry(_)));
    assert!(started.elapsed() >= runner.invocation_bound);
    assert_eq!(stored(&setup).await.ownership.until, fixture::NOW + 20);
    assert!(matches!(
        stored(&setup).await.phase,
        Phase::Accepted {
            durable_cursor: Some(0),
            ..
        }
    ));
}

#[tokio::test]
async fn persisted_pending_terminal_effect_and_settled_receipt_survive_crashes() {
    let setup = Setup::new(supported()).await;
    let http = HttpServer::start().await;
    http.memory.lock().unwrap().config.replay_frames = Some(frames());
    let (mut runner, mut rx, work) = executor(&setup, &http);
    runner.frame_limit = 3;
    assert!(matches!(runner.execute(&work).await, Disposition::Retry(_)));
    let mut lease = setup
        .journal()
        .claim(&setup.plan, setup.target(), "crash-fixture")
        .await
        .unwrap();
    let body = setup.journal().body(&lease).await.unwrap();
    let mut projection = setup
        .journal()
        .load_projection(&lease)
        .await
        .unwrap()
        .unwrap();
    let raw = frames().pop().unwrap();
    let frame = crate::providers::RecoveryEvent {
        trace_contract: false,
        trace: None,
        response_id: "resp_accepted".into(),
        sequence_number: 3,
        event: crate::providers::parse_event(&crate::providers::SseFrame {
            event: None,
            id: Some("3".into()),
            data: raw.to_string(),
            retry: None,
        })
        .unwrap(),
        raw,
    };
    projection.project(&frame, &body, false, "failed").unwrap();
    setup
        .journal()
        .save_projection(&mut lease, &projection)
        .await
        .unwrap();
    setup.journal().release(&mut lease).await.unwrap();
    runner.frame_limit = 4096;
    let (result, ()) = tokio::join!(runner.execute(&work), async {
        let reply = command(&mut rx).await;
        assert_eq!(reply.message.text.as_deref(), Some("hello world"));
        reply
            .receipt
            .send(receipt_for(DeliveryOutcome::Confirmed {
                platform_message_id: Some("one".into()),
            }))
            .unwrap();
    });
    assert!(matches!(result, Disposition::Settled));
    setup.server.change(&setup.target().id, |state| {
        state["phase"]["state"] = json!("accepted");
        state["phase"]["durable_cursor"] = json!(2);
    });
    assert!(matches!(runner.execute(&work).await, Disposition::Settled));
    assert!(matches!(
        stored(&setup).await.phase,
        Phase::Completed {
            durable_cursor: Some(3),
            ..
        }
    ));
    assert!(rx.try_recv().is_err());
    assert_eq!(http.seen().iter().filter(|v| v.method == "POST").count(), 1);
}

#[path = "readiness_acknowledgement_tests.rs"]
mod readiness_acknowledgement;
