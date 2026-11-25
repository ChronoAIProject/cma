use super::*;

#[tokio::test]
async fn actual_post_refusal_then_expiry_preserves_first_write_without_reposting() {
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
    assert_eq!(first.post_attempts, 1);
    assert!(first.accepted_ns.is_none());
    setup
        .server
        .memory
        .clock
        .store(setup.target().launch.event.expires_at, Ordering::SeqCst);
    assert!(matches!(
        dispatcher.dispatch(&setup.plan.identity, &setup.target().id, "expired").await.unwrap(),
        DispatchOutcome::Terminal(Phase::Refused { code }) if code == super::super::expiry::EXPIRED_BEFORE_START
    ));
    let snapshot = m.snapshot();
    let row = &snapshot.samples[0];
    assert_eq!(row.first_post_ns, first.first_post_ns);
    assert_eq!(row.post_attempts, 1);
    assert_eq!(http.seen().len(), 1);
    assert!(row.expired_before_start_ns.is_none(),
        "a proved non-admitting POST followed by expiry must not be labeled expired before first POST");
    assert!(row.expired_after_post_ns.unwrap() >= first.first_post_ns.unwrap());
    assert!(row.accepted_ns.is_none());
    assert!(row.completed_ns.is_none());
    assert!(row.unknown.is_none());
    eprintln!(
        "PROVIDER_START_RETRY_EXPIRY {}",
        serde_json::to_string(&snapshot).unwrap()
    );
    let terminal = row.expired_after_post_ns;
    dispatcher
        .dispatch(&setup.plan.identity, &setup.target().id, "replay")
        .await
        .unwrap();
    assert_eq!(m.snapshot().samples[0].expired_after_post_ns, terminal);
    assert_eq!(http.seen().len(), 1);
}

#[tokio::test]
async fn expiry_without_original_post_observation_is_unknown_even_with_known_admission() {
    for retained in [false, true] {
        let original = Measurement::new(4).unwrap();
        let setup = setup(Some(original.clone())).await;
        let http = HttpServer::start().await;
        {
            let mut state = http.memory.lock().unwrap();
            state.config.post_status = 429;
            state.config.post_error_body = Some(
                json!({"error":{"type":"invalid_request_error","code":"conversation_not_ready"}})
                    .to_string(),
            );
        }
        assert!(matches!(
            http_port(&setup, &http, None)
                .dispatcher()
                .dispatch(&setup.plan.identity, &setup.target().id, "first")
                .await
                .unwrap(),
            DispatchOutcome::Retry
        ));
        let restarted = if retained {
            original
        } else {
            Measurement::new(4).unwrap()
        };
        if !retained {
            let request = request();
            let candidate = Candidate::prepare(
                &request,
                &fixture::routed(&request),
                b"synthetic",
                &fixture::keys(),
                fixture::NOW,
            )
            .unwrap();
            setup
                .server
                .store()
                .with_start_measurement(restarted.clone())
                .admit(&candidate)
                .await
                .unwrap();
        }
        setup
            .server
            .memory
            .clock
            .store(setup.target().launch.event.expires_at, Ordering::SeqCst);
        assert!(matches!(
            http_port(&setup, &http, Some(restarted.clone()))
                .dispatcher()
                .dispatch(&setup.plan.identity, &setup.target().id, "expired")
                .await
                .unwrap(),
            DispatchOutcome::Terminal(Phase::Refused { .. })
        ));
        let row = restarted.snapshot().samples.remove(0);
        assert!(row.admitted);
        assert!(row.first_post_ns.is_none());
        assert_eq!(row.post_attempts, 0);
        assert!(row.expired_before_start_ns.is_none());
        assert!(row.expired_after_post_ns.is_none());
        assert_eq!(row.admission_confirmed_ns.is_some(), retained);
        assert_eq!(
            row.unknown,
            Some(if retained {
                "first_post_observation_missing"
            } else {
                "retained_or_uncertain_admission"
            })
        );
        assert_eq!(http.seen().len(), 1);
    }
}

#[tokio::test]
async fn retry_after_another_dispatcher_post_is_not_a_first_write_sample() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let _guard = metrics::set_default_local_recorder(&recorder);
    let measurement = Measurement::new(4).unwrap();
    let setup = setup(Some(measurement.clone())).await;
    let http = HttpServer::start().await;
    {
        let mut state = http.memory.lock().unwrap();
        state.config.post_status = 429;
        state.config.post_error_body = Some(
            json!({"error":{"type":"invalid_request_error","code":"conversation_not_ready"}})
                .to_string(),
        );
    }
    let identity = &setup.plan.identity;
    let target = &setup.target().id;
    assert!(matches!(
        http_port(&setup, &http, None)
            .dispatcher()
            .dispatch(identity, target, "remote-owner")
            .await
            .unwrap(),
        DispatchOutcome::Retry
    ));
    {
        let mut state = http.memory.lock().unwrap();
        state.config.post_status = 200;
        state.config.post_error_body = None;
    }
    assert!(matches!(
        http_port(&setup, &http, Some(measurement.clone()))
            .dispatcher()
            .dispatch(identity, target, "admission-owner")
            .await
            .unwrap(),
        DispatchOutcome::Replay { .. }
    ));
    assert_eq!(http.seen().len(), 2);
    let row = measurement.snapshot().samples.remove(0);
    assert!(row.admission_confirmed_ns.is_some());
    assert_eq!(row.first_post_ns, None);
    assert_eq!(row.post_attempts, 1);
    assert!(row.accepted_attempt_post_ns.is_some());
    assert_eq!(row.unknown, Some("first_post_observation_missing"));
    assert!(!recorder
        .handle()
        .render()
        .contains(crate::telemetry::names::PROVIDER_START_LAG));
}

#[tokio::test]
async fn local_candidate_age_cannot_prove_waiting_across_dispatchers() {
    use crate::telemetry::names::PROVIDER_START_OLDEST_LOCAL_CANDIDATE as AGE;

    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let _guard = metrics::set_default_local_recorder(&recorder);
    let measurement = Measurement::new(4).unwrap();
    let setup = setup(Some(measurement.clone())).await;
    let http = HttpServer::start().await;
    let observe = || {
        measurement.record_window_metrics();
        let output = recorder.handle().render();
        assert!(output.contains(&format!("{AGE}{{bound=\"lower\"}} NaN")));
        let prefix = format!("{AGE}{{bound=\"upper\"}} ");
        output
            .lines()
            .find_map(|line| line.strip_prefix(&prefix))
            .unwrap()
            .parse::<f64>()
            .unwrap()
    };
    let before = observe();
    let remote = http_port(&setup, &http, None).dispatcher();
    let DispatchOutcome::Replay {
        mut lease,
        response_id,
        ..
    } = remote
        .dispatch(&setup.plan.identity, &setup.target().id, "remote-owner")
        .await
        .unwrap()
    else {
        panic!("remote owner must accept the POST")
    };
    acknowledge_no_effect(&setup, &mut lease, &response_id, 0, true).await;
    remote
        .journal()
        .complete(&mut lease, &response_id)
        .await
        .unwrap();
    remote.journal().release(&mut lease).await.unwrap();
    assert_eq!(http.seen().len(), 1);
    assert!(measurement.snapshot().samples[0].completed_ns.is_none());
    let after = observe();
    assert!(before.is_finite() && after.is_finite() && after >= before);
}
