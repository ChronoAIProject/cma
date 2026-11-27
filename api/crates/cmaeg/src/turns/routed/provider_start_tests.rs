use super::{adapter_test_server::HttpServer, test_support::*, *};
use crate::{ingress::routing_store::Candidate, telemetry::provider_start::Measurement};
use serde_json::json;
use std::{
    sync::{atomic::Ordering, Arc},
    time::Duration,
};

mod qualification;
mod retry_expiry;

fn request() -> crate::ingress::launch::LaunchRequest {
    fixture::request(
        |v| {
            v["routing"]["default_targets"] = json!(["default"]);
            v["reply"]["sinks"] = json!([]);
            v["reply"]["auto_final"] = false.into();
        },
        fixture::custom(None),
    )
}

async fn setup(measurement: Option<Measurement>) -> Setup {
    let request = request();
    let server = crate::ingress::routing_store::test_server::Server::start().await;
    let mut plans = server.store();
    if let Some(m) = measurement {
        plans = plans.with_start_measurement(m);
    }
    let candidate = Candidate::prepare(
        &request,
        &fixture::routed(&request),
        b"synthetic",
        &fixture::keys(),
        fixture::NOW,
    )
    .unwrap();
    let plan = plans.admit(&candidate).await.unwrap();
    assert_eq!(plan.targets.len(), 1);
    let mut journal = Journal::new(server.records.clone(), Arc::new(fixture::keys()));
    let clock = server.memory.clock.clone();
    journal.clock = Arc::new(move || clock.load(Ordering::SeqCst));
    let current = Arc::new(Current {
        channel: std::sync::Mutex::new(request.channel),
        calls: 0.into(),
        revoke_at: usize::MAX.into(),
    });
    let port = Arc::new(Port {
        starts: std::sync::Mutex::new(vec![]),
        prepares: 0.into(),
        support: supported(),
        next: std::sync::Mutex::new(None),
        input_size: 32,
    });
    let dispatcher = Dispatcher::new(plans, journal, current.clone(), port.clone());
    Setup {
        server,
        plan,
        current,
        port,
        dispatcher,
    }
}
fn http_port(
    setup: &Setup,
    http: &HttpServer,
    measurement: Option<Measurement>,
) -> Arc<HttpExecutionPort> {
    let mut plans = setup.server.store();
    if let Some(m) = measurement {
        plans = plans.with_start_measurement(m);
    }
    Arc::new(HttpExecutionPort::new(
        http.client(true, Duration::from_secs(2)),
        setup.port.clone(),
        plans,
        setup.journal().clone(),
        setup.current.clone(),
    ))
}

#[tokio::test]
async fn frozen_partition_refusal_is_terminal_only_for_the_documented_response() {
    for (status, code) in [
        (409, "partition_agent_unavailable"),
        (410, "partition_agent_missing"),
    ] {
        for fault in [
            None,
            Some("code"),
            Some("status"),
            Some("param"),
            Some("receipt"),
        ] {
            let setup = setup(None).await;
            let http = HttpServer::start().await;
            let mut problem = json!({"error": {
                "type": "invalid_request_error",
                "code": code,
                "message": "The binding Agent cannot accept input.",
                "param": "conversation"
            }});
            {
                let mut memory = http.memory.lock().unwrap();
                let config = &mut memory.config;
                config.post_status = match fault {
                    Some("status") if status == 409 => 410,
                    Some("status") => 409,
                    _ => status,
                };
                match fault {
                    Some("code") => problem["error"]["code"] = "other_conflict".into(),
                    Some("param") => problem["error"]["param"] = "input".into(),
                    Some("receipt") => config.post_error_response_id = true,
                    _ => {}
                }
                config.post_error_body = Some(problem.to_string());
            }
            let dispatcher = http_port(&setup, &http, None).dispatcher();
            let observed = dispatcher
                .dispatch(&setup.plan.identity, &setup.target().id, "first")
                .await
                .unwrap();
            if fault.is_none() {
                assert!(matches!(
                    observed,
                    DispatchOutcome::Terminal(Phase::Refused { code: observed }) if observed == code
                ));
                assert!(matches!(
                    dispatcher
                        .dispatch(&setup.plan.identity, &setup.target().id, "again")
                        .await
                        .unwrap(),
                    DispatchOutcome::Terminal(Phase::Refused { code: observed }) if observed == code
                ));
            } else {
                assert!(
                    matches!(observed, DispatchOutcome::Unknown),
                    "{status} {fault:?}"
                );
            }
            assert_eq!(http.seen().len(), 1);
        }
    }
}

#[tokio::test]
async fn real_post_and_terminal_are_distinct_from_task_start_and_retry() {
    let measurement = Measurement::new(4).unwrap();
    let setup = setup(Some(measurement.clone())).await;
    let http = HttpServer::start().await;
    let port = http_port(&setup, &http, Some(measurement.clone()));
    let dispatcher = port.clone().dispatcher();
    let mut executing = measurement.executor(&setup.plan.admission_id);
    assert!(measurement.snapshot().samples[0].first_post_ns.is_none());
    let DispatchOutcome::Replay {
        mut lease,
        response_id,
        ..
    } = dispatcher
        .dispatch(&setup.plan.identity, &setup.target().id, "measured")
        .await
        .unwrap()
    else {
        panic!("accepted")
    };
    let first = measurement.snapshot();
    let row = &first.samples[0];
    assert!(row.commit_lower_ns.unwrap() <= row.admission_confirmed_ns.unwrap());
    assert!(row.admission_confirmed_ns.unwrap() <= row.first_post_ns.unwrap());
    assert!(row.first_post_ns.unwrap() <= row.accepted_ns.unwrap());
    assert!(row.completed_ns.is_none());
    acknowledge_no_effect(&setup, &mut lease, &response_id, 0, true).await;
    dispatcher
        .journal()
        .complete(&mut lease, &response_id)
        .await
        .unwrap();
    dispatcher.journal().release(&mut lease).await.unwrap();
    executing.finished();
    drop(executing);
    let _ = dispatcher
        .dispatch(&setup.plan.identity, &setup.target().id, "retry")
        .await
        .unwrap();
    let final_sample = measurement.snapshot();
    assert_eq!(final_sample.samples[0].post_attempts, 1);
    assert!(final_sample.samples[0].completed_ns.is_some());
    assert_eq!(http.seen().iter().filter(|r| r.method == "POST").count(), 1);
    assert_eq!(final_sample.active_executors, 0);
}

#[tokio::test]
async fn guard_refusal_and_expiry_do_not_invent_a_post() {
    for expired in [false, true] {
        let m = Measurement::new(4).unwrap();
        let setup = setup(Some(m.clone())).await;
        let http = HttpServer::start().await;
        let port = http_port(&setup, &http, Some(m.clone()));
        if expired {
            setup
                .server
                .memory
                .clock
                .store(setup.target().launch.event.expires_at, Ordering::SeqCst);
        } else {
            setup.current.revoke_at.store(1, Ordering::SeqCst);
        }
        let result = port
            .dispatcher()
            .dispatch(&setup.plan.identity, &setup.target().id, "refused")
            .await;
        if expired {
            assert!(matches!(result, Ok(DispatchOutcome::Terminal(_))));
        } else {
            assert!(matches!(result, Err(ExecutionError::AuthorityChanged)));
        }
        let rows = m.snapshot().samples;
        assert!(rows[0].first_post_ns.is_none());
        assert_eq!(rows[0].expired_before_start_ns.is_some(), expired);
        assert!(http.seen().is_empty());
    }
}

#[tokio::test]
async fn lost_acceptance_and_cancellation_keep_write_evidence_without_reposting() {
    let m = Measurement::new(4).unwrap();
    let setup = setup(Some(m.clone())).await;
    let http = HttpServer::start().await;
    let posted = Arc::new(tokio::sync::Notify::new());
    http.memory.lock().unwrap().config.post_idle = true;
    let notified = posted.clone();
    http.memory.lock().unwrap().config.on_post = Some(Arc::new(move || notified.notify_one()));
    let port = http_port(&setup, &http, Some(m.clone()));
    let dispatcher = port.clone().dispatcher();
    let mut pending =
        Box::pin(dispatcher.dispatch(&setup.plan.identity, &setup.target().id, "cancelled"));
    tokio::time::timeout(Duration::from_secs(5), async {
        tokio::select! { _ = posted.notified() => {}, _ = &mut pending => panic!("acceptance must remain pending") }
    }).await.unwrap();
    drop(pending);
    let rows = m.snapshot().samples;
    assert!(rows[0].first_post_ns.is_some());
    assert!(rows[0].accepted_ns.is_none());
    setup.server.memory.clock.fetch_add(31, Ordering::SeqCst);
    assert!(matches!(
        dispatcher
            .dispatch(&setup.plan.identity, &setup.target().id, "retry")
            .await
            .unwrap(),
        DispatchOutcome::Unknown
    ));
    assert_eq!(http.seen().len(), 1);
    assert_eq!(m.snapshot().samples[0].post_attempts, 1);
    let restarted = Measurement::new(4).unwrap();
    let plans = setup
        .server
        .store()
        .with_start_measurement(restarted.clone());
    let request = request();
    let candidate = Candidate::prepare(
        &request,
        &fixture::routed(&request),
        b"synthetic",
        &fixture::keys(),
        fixture::NOW,
    )
    .unwrap();
    plans.admit(&candidate).await.unwrap();
    assert_eq!(
        restarted.snapshot().samples[0].unknown,
        Some("retained_or_uncertain_admission")
    );
    assert!(restarted.snapshot().samples[0].first_post_ns.is_none());
}

#[tokio::test]
async fn actual_retry_keeps_first_write_and_original_identity() {
    let m = Measurement::new(4).unwrap();
    let setup = setup(Some(m.clone())).await;
    let http = HttpServer::start().await;
    {
        let mut state = http.memory.lock().unwrap();
        state.config.post_status = 429;
        state.config.post_error_body = Some(
            json!({"error":{"type":"invalid_request_error","code":"conversation_not_ready"}})
                .to_string(),
        );
    }
    let dispatcher = http_port(&setup, &http, Some(m.clone())).dispatcher();
    assert!(matches!(
        dispatcher
            .dispatch(&setup.plan.identity, &setup.target().id, "first")
            .await
            .unwrap(),
        DispatchOutcome::Retry
    ));
    let first = m.snapshot().samples.remove(0);
    assert!(first.first_post_ns.is_some());
    assert!(first.accepted_ns.is_none());
    http.memory.lock().unwrap().config.post_status = 200;
    assert!(matches!(
        dispatcher
            .dispatch(&setup.plan.identity, &setup.target().id, "retry")
            .await
            .unwrap(),
        DispatchOutcome::Replay { .. }
    ));
    let rows = m.snapshot().samples;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].first_post_ns, first.first_post_ns);
    assert_eq!(rows[0].post_attempts, 2);
    assert!(rows[0].accepted_ns.is_some());
    let seen = http.seen();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].idempotency, seen[1].idempotency);
    assert_eq!(seen[0].body, seen[1].body);
}

#[tokio::test]
async fn ambiguous_admission_readback_does_not_claim_a_commit_instant() {
    let m = Measurement::new(4).unwrap();
    let server = crate::ingress::routing_store::test_server::Server::start().await;
    let plans = server.store().with_start_measurement(m.clone());
    let request = request();
    let candidate = Candidate::prepare(
        &request,
        &fixture::routed(&request),
        b"synthetic",
        &fixture::keys(),
        fixture::NOW,
    )
    .unwrap();
    server.fault(candidate.identity().admission_id(), true, false);
    plans.admit(&candidate).await.unwrap();
    let row = m.snapshot().samples.remove(0);
    assert!(row.admitted);
    assert!(row.commit_lower_ns.is_none());
    assert!(row.admission_confirmed_ns.is_none());
    assert_eq!(row.candidate_admitted_at_seconds, Some(fixture::NOW));
    assert_eq!(row.unknown, Some("retained_or_uncertain_admission"));
}

#[tokio::test]
async fn default_construction_has_no_measurement_and_preserves_dispatch() {
    let setup = setup(None).await;
    assert!(setup.server.store().start_measurement.is_none());
    assert!(setup.journal().start_measurement.is_none());
    let http = HttpServer::start().await;
    let port = http_port(&setup, &http, None);
    assert!(matches!(
        port.dispatcher()
            .dispatch(&setup.plan.identity, &setup.target().id, "off")
            .await
            .unwrap(),
        DispatchOutcome::Replay { .. }
    ));
    assert_eq!(http.seen().len(), 1);
}
