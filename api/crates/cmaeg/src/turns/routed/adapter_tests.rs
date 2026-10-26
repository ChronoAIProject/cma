use super::{adapter_test_server::HttpServer, test_support::*, *};
use crate::{
    channels::resolver::ResolvedChannel,
    ingress::routing_store::{AdmittedPlan, StoredTarget},
};
use cmaeg_core::channel::Secret;
use futures_util::future::BoxFuture;
use std::{
    sync::{atomic::Ordering, Arc},
    time::Duration,
};

impl FrozenTurnBuilder for Port {
    fn prepare<'a>(
        &'a self,
        plan: &'a AdmittedPlan,
        target: &'a StoredTarget,
        current: &'a ResolvedChannel,
    ) -> BoxFuture<'a, Result<FrozenTurn, ExecutionError>> {
        ExecutionPort::prepare(self, plan, target, current)
    }
}
fn http_port(setup: &Setup, http: &HttpServer, recovery: bool) -> Arc<HttpExecutionPort> {
    Arc::new(HttpExecutionPort::new(
        http.client(recovery, Duration::from_millis(250)),
        setup.port.clone(),
        setup.server.store(),
        setup.journal().clone(),
        setup.current.clone(),
    ))
}
async fn dispatch(
    setup: &Setup,
    port: &Arc<HttpExecutionPort>,
) -> Result<DispatchOutcome, ExecutionError> {
    port.clone()
        .dispatcher()
        .dispatch(&setup.plan.identity, &setup.target().id, "http-worker")
        .await
}
fn rotate(setup: &Setup) {
    let mut current = setup.current.channel.lock().unwrap();
    let definition = &mut Arc::get_mut(&mut current).unwrap().definition;
    definition
        .secrets
        .consumer_keys
        .iter_mut()
        .find(|(id, _)| id == &setup.target().launch.consumer)
        .unwrap()
        .1 = Secret::new("rotated-fixture");
}

#[tokio::test]
async fn exact_frozen_post_accepts_then_restarts_with_get_only_and_fresh_credentials() {
    let setup = Setup::new(supported()).await;
    let http = HttpServer::start().await;
    let port = http_port(&setup, &http, true);
    assert!(
        !port
            .support(&setup.target().launch.provider)
            .exact_body_idempotency
    );
    let DispatchOutcome::Replay {
        mut lease,
        body,
        response_id,
        durable_cursor,
    } = dispatch(&setup, &port).await.unwrap()
    else {
        panic!("accepted")
    };
    assert_eq!(durable_cursor, None);
    assert_eq!(response_id, "resp_accepted");
    let seen = http.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].method, "POST");
    assert_eq!(seen[0].body, body.provider_body());
    assert_eq!(
        seen[0].idempotency.as_deref(),
        Some(body.idempotency_key.as_str())
    );
    assert!(!String::from_utf8_lossy(&seen[0].body).contains("rotated-fixture"));
    setup.journal().release(&mut lease).await.unwrap();
    rotate(&setup);
    let DispatchOutcome::Replay {
        mut lease,
        durable_cursor,
        ..
    } = dispatch(&setup, &port).await.unwrap()
    else {
        panic!("reattach")
    };
    assert_eq!(durable_cursor, None);
    assert_eq!(setup.port.prepares.load(Ordering::SeqCst), 1);
    let mut attached = port
        .reattach(&setup.plan.identity, &setup.target().id, &lease, None)
        .await
        .unwrap();
    assert_eq!(attached.snapshot.native_agent_id, "agt_native");
    for expected in 0..=2 {
        let event = attached.stream.events.recv().await.unwrap().unwrap();
        assert_eq!(event.sequence_number, expected);
        // Observation alone never changes the journal's committed cursor.
        assert!(matches!(
            lease.state().phase,
            Phase::Accepted {
                durable_cursor: None,
                ..
            }
        ));
    }
    assert!(attached.stream.events.recv().await.is_none());
    let seen = http.seen();
    assert_eq!(seen.len(), 3);
    assert!(seen[1..]
        .iter()
        .all(|request| request.method == "GET" && request.bearer == "Bearer rotated-fixture"));
    assert!(!seen[2].uri.contains("starting_after"));
    acknowledge_no_effect(&setup, &mut lease, &response_id, 0, false).await;
    let mut attached = port
        .reattach(
            &setup.plan.identity,
            &setup.target().id,
            &lease,
            Some("agt_native"),
        )
        .await
        .unwrap();
    assert_eq!(
        attached
            .stream
            .events
            .recv()
            .await
            .unwrap()
            .unwrap()
            .sequence_number,
        1
    );
    drop(attached);
    assert!(http
        .seen()
        .last()
        .unwrap()
        .uri
        .ends_with("?starting_after=0"));
}

#[tokio::test]
async fn uncertain_post_never_reposts_or_regenerates_frozen_body() {
    for status in [409, 429, 503] {
        let setup = Setup::new(supported()).await;
        let http = HttpServer::start().await;
        http.memory.lock().unwrap().config.post_status = status;
        let port = http_port(&setup, &http, true);
        assert!(matches!(
            dispatch(&setup, &port).await.unwrap(),
            DispatchOutcome::Unknown
        ));
        assert!(matches!(
            dispatch(&setup, &port).await.unwrap(),
            DispatchOutcome::Unknown
        ));
        assert_eq!(http.seen().len(), 1);
        assert_eq!(setup.port.prepares.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn cancellation_is_not_a_detached_start_and_recovered_starting_stays_unknown() {
    let setup = Setup::new(supported()).await;
    let http = HttpServer::start().await;
    http.memory.lock().unwrap().config.post_idle = true;
    let port = http_port(&setup, &http, true);
    assert!(
        tokio::time::timeout(Duration::from_millis(80), dispatch(&setup, &port))
            .await
            .is_err()
    );
    assert_eq!(http.seen().len(), 1);
    setup.server.memory.clock.fetch_add(31, Ordering::SeqCst);
    assert!(matches!(
        dispatch(&setup, &port).await.unwrap(),
        DispatchOutcome::Unknown
    ));
    assert_eq!(http.seen().len(), 1);
    // A fresh target can use the same provider slot after the cancelled future.
    let target = &setup.plan.targets[1];
    let body = frozen(
        &setup.plan,
        target,
        &setup.current.channel.lock().unwrap(),
        1,
    );
    http.memory.lock().unwrap().config.post_idle = false;
    let result = port
        .start(&body, &target.launch.provider, &Secret::new("fixture-key"))
        .await;
    assert!(matches!(result, StartResult::Accepted { .. }));
}

#[tokio::test]
async fn invalid_acceptance_is_unknown_and_proved_json_rejection_is_terminal() {
    let setup = Setup::new(supported()).await;
    let http = HttpServer::start().await;
    http.memory.lock().unwrap().config.corrupt_acceptance = true;
    let port = http_port(&setup, &http, true);
    assert!(matches!(
        dispatch(&setup, &port).await.unwrap(),
        DispatchOutcome::Unknown
    ));
    assert!(matches!(
        dispatch(&setup, &port).await.unwrap(),
        DispatchOutcome::Unknown
    ));
    assert_eq!(http.seen().len(), 1);
    let setup = Setup::new(supported()).await;
    let http = HttpServer::start().await;
    http.memory.lock().unwrap().config.post_status = 400;
    let port = http_port(&setup, &http, true);
    assert!(matches!(
        dispatch(&setup, &port).await.unwrap(),
        DispatchOutcome::Terminal(Phase::Refused { .. })
    ));
    assert!(matches!(
        dispatch(&setup, &port).await.unwrap(),
        DispatchOutcome::Terminal(Phase::Refused { .. })
    ));
    assert_eq!(http.seen().len(), 1);
}

#[tokio::test]
async fn missing_recovery_and_changed_authority_never_start_http_work() {
    let setup = Setup::new(supported()).await;
    let http = HttpServer::start().await;
    let port = http_port(&setup, &http, false);
    assert!(matches!(
        dispatch(&setup, &port).await,
        Err(ExecutionError::Unsupported)
    ));
    assert!(http.seen().is_empty());
    assert_eq!(setup.port.prepares.load(Ordering::SeqCst), 0);
    let port = http_port(&setup, &http, true);
    setup.current.revoke_at.store(1, Ordering::SeqCst);
    assert!(matches!(
        dispatch(&setup, &port).await,
        Err(ExecutionError::AuthorityChanged)
    ));
    assert!(http.seen().is_empty());
}

#[tokio::test]
async fn replay_revalidates_authority_between_snapshot_and_events() {
    let setup = Setup::new(supported()).await;
    let http = HttpServer::start().await;
    let port = http_port(&setup, &http, true);
    let DispatchOutcome::Replay { lease, .. } = dispatch(&setup, &port).await.unwrap() else {
        panic!("accepted")
    };
    setup.current.revoke_at.store(
        setup.current.calls.load(Ordering::SeqCst) + 2,
        Ordering::SeqCst,
    );
    let result = port
        .reattach(&setup.plan.identity, &setup.target().id, &lease, None)
        .await;
    assert!(matches!(
        result,
        Err(ReattachError::Execution(ExecutionError::AuthorityChanged))
    ));
    let seen = http.seen();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[1].method, "GET");
    assert!(!seen[1].uri.ends_with("/events"));
}

#[tokio::test]
async fn replay_rechecks_lease_after_snapshot_and_never_mutates_cursor() {
    let setup = Setup::new(supported()).await;
    let http = HttpServer::start().await;
    let port = http_port(&setup, &http, true);
    let DispatchOutcome::Replay { lease, .. } = dispatch(&setup, &port).await.unwrap() else {
        panic!("accepted")
    };
    let clock = setup.server.memory.clock.clone();
    http.memory.lock().unwrap().config.on_snapshot = Some(Arc::new(move || {
        clock.fetch_add(31, Ordering::SeqCst);
    }));
    assert!(matches!(
        port.reattach(&setup.plan.identity, &setup.target().id, &lease, None)
            .await,
        Err(ReattachError::Execution(ExecutionError::LostLease))
    ));
    assert_eq!(http.seen().len(), 2);
    assert!(matches!(
        lease.state().phase,
        Phase::Accepted {
            durable_cursor: None,
            ..
        }
    ));
}

#[tokio::test]
async fn replay_rejects_foreign_targets_and_native_agent_substitution() {
    let setup = Setup::new(supported()).await;
    let http = HttpServer::start().await;
    let port = http_port(&setup, &http, true);
    let DispatchOutcome::Replay { lease, .. } = dispatch(&setup, &port).await.unwrap() else {
        panic!("accepted")
    };
    assert!(matches!(
        port.reattach(
            &setup.plan.identity,
            &setup.plan.targets[1].id,
            &lease,
            None
        )
        .await,
        Err(ReattachError::Execution(ExecutionError::Invalid))
    ));
    assert_eq!(http.seen().len(), 1);
    assert!(matches!(
        port.reattach(
            &setup.plan.identity,
            &setup.target().id,
            &lease,
            Some("agt_other")
        )
        .await,
        Err(ReattachError::Provider(
            crate::providers::ProviderError::ResponseMalformed(_)
        ))
    ));
    assert_eq!(http.seen().len(), 2);
}

#[tokio::test]
async fn malformed_acceptance_protocol_never_permits_another_post() {
    for fault in [
        "content_type",
        "missing_identity",
        "duplicate_identity",
        "sequence",
    ] {
        let setup = Setup::new(supported()).await;
        let http = HttpServer::start().await;
        http.memory.lock().unwrap().config.acceptance_fault = Some(fault);
        let port = http_port(&setup, &http, true);
        for _ in 0..2 {
            assert!(
                matches!(
                    dispatch(&setup, &port).await.unwrap(),
                    DispatchOutcome::Unknown
                ),
                "{fault}"
            );
        }
        assert_eq!(http.seen().len(), 1, "{fault}");
        assert_eq!(setup.port.prepares.load(Ordering::SeqCst), 1, "{fault}");
    }
}
