use super::*;

#[tokio::test(start_paused = true)]
async fn revoked_share_closes_before_buffered_live_or_idle_fallback() {
    for live in [false, true] {
        let f = fixture(true, true).await;
        let share = share(&f).await;
        let auth = AuthenticatedResourceCaller {
            context: context("reader"),
            credential: Some(crate::DelegatedCredential::new("fixture-credential".into())),
        };
        let mut open = open(
            f.state
                .clone()
                .with_event_wake(true, Duration::from_secs(1)),
            auth,
            AgentId("agt_fixture".into()),
            None,
            HeaderMap::new(),
        )
        .await
        .unwrap();
        open.pending.clear();
        open.initial_gap = false;
        if live {
            publish(&open, "withheld").await;
        }
        f.access
            .change_share(
                &context("owner-a"),
                None,
                ShareCommand {
                    selector: share.selector,
                    change: ShareChange::Revoke { share_id: share.id },
                    expected_share_revision: Some(share.revision),
                    idempotency_key: "wake-revoke".into(),
                },
            )
            .await
            .unwrap();
        let start = Instant::now();
        let mut body = response(open).into_body().into_data_stream();
        assert!(body.next().await.is_none());
        assert!(start.elapsed() <= Duration::from_secs(1));
    }
}

#[tokio::test(start_paused = true)]
async fn authorization_deadline_preempts_a_long_fallback_and_buffered_batch() {
    for buffered in [false, true] {
        let f = fixture(true, true).await;
        share(&f).await;
        let auth = AuthenticatedResourceCaller {
            context: context("reader"),
            credential: Some(crate::DelegatedCredential::new("fixture-credential".into())),
        };
        let mut open = open(
            f.state.with_event_wake(true, Duration::from_secs(120)),
            auth,
            AgentId("agt_fixture".into()),
            None,
            HeaderMap::new(),
        )
        .await
        .unwrap();
        open.pending.clear();
        open.initial_gap = false;
        let deadline = open.authorization.revalidation_deadline().unwrap();
        if buffered {
            publish(&open, "expired batch").await;
            open.wait_for_change().await.unwrap();
            assert!(!open.pending.is_empty());
            tokio::time::advance(deadline - Instant::now()).await;
        }
        let mut body = response(open).into_body().into_data_stream();
        while let Some(frame) = body.next().await {
            assert!(!String::from_utf8_lossy(&frame.unwrap()).contains("event:"));
        }
        assert_eq!(Instant::now(), deadline);
    }
}

#[tokio::test(start_paused = true)]
async fn runtime_race_gaps_stale_fragments_after_durable_catchup() {
    let (open, f, _) = prepared(true).await;
    publish(&open, "stale runtime").await;
    append(
        &f.store,
        AgentLogKind::OwnerMessage {
            content: "durable before gap".into(),
        },
    )
    .await;
    let mut agent = f.store.get_agent(&open.header.id).await.unwrap().unwrap();
    let revision = agent.revision;
    agent.conversation.runtime_generation = Some(2);
    agent.revision += 1;
    f.store.save_agent(revision, agent).await.unwrap();
    let mut body = response(open).into_body().into_data_stream();
    assert!(next_wire(&mut body).await.contains("durable before gap"));
    let gap = next_wire(&mut body).await;
    assert!(gap.contains("cma.agent.live_gap") && gap.contains("lagged"));
    assert!(!gap.contains("stale runtime"));
}

#[tokio::test(start_paused = true)]
async fn lagged_subscription_recovers_durably_then_reports_gap() {
    let (open, f, _) = prepared(true).await;
    for _ in 0..300 {
        publish(&open, "retained live").await;
    }
    append(
        &f.store,
        AgentLogKind::OwnerMessage {
            content: "durable catchup".into(),
        },
    )
    .await;
    let mut body = response(open).into_body().into_data_stream();
    assert!(next_wire(&mut body).await.contains("durable catchup"));
    assert!(next_wire(&mut body).await.contains("cma.agent.live_gap"));
    assert!(next_wire(&mut body).await.contains("retained live"));
    append(
        &f.store,
        AgentLogKind::OwnerMessage {
            content: "later durable".into(),
        },
    )
    .await;
    loop {
        let wire = next_wire(&mut body).await;
        if wire.contains("later durable") {
            break;
        }
        assert!(wire.contains("cma.agent.live"));
    }
}

#[tokio::test(start_paused = true)]
async fn live_wakes_do_not_postpone_fixed_fallback() {
    let (mut open, _, probe) = prepared(true).await;
    let due = open.fallback_due;
    for _ in 0..9 {
        tokio::time::advance(Duration::from_millis(100)).await;
        publish(&open, "active").await;
        open.wait_for_change().await.unwrap();
        open.pending.clear();
        assert_eq!(open.fallback_due, due);
    }
    open.wait_for_change().await.unwrap();
    assert_eq!(Instant::now(), due);
    assert_eq!(probe.snapshots.load(Ordering::Relaxed), 10);
    assert_eq!(probe.logs.load(Ordering::Relaxed), 0);
}

#[tokio::test(start_paused = true)]
async fn disabled_switch_keeps_hundred_ms_poll_and_log_reads() {
    let (open, _, probe) = prepared(false).await;
    publish(&open, "legacy poll").await;
    let start = Instant::now();
    let mut body = response(open).into_body().into_data_stream();
    assert!(next_wire(&mut body).await.contains("legacy poll"));
    assert_eq!(start.elapsed(), Duration::from_millis(100));
    assert_eq!(probe.snapshots.load(Ordering::Relaxed), 4);
    assert!(probe.logs.load(Ordering::Relaxed) > 0);
}

#[tokio::test(start_paused = true)]
async fn hundred_idle_streams_have_no_log_reads_and_one_snapshot_per_second_each() {
    let (first, f, probe) = prepared(true).await;
    let state = first.state.clone();
    let mut streams = vec![first];
    for _ in 1..100 {
        let auth = AuthenticatedResourceCaller {
            context: context("owner-a"),
            credential: None,
        };
        let mut stream = open(
            state.clone(),
            auth,
            AgentId("agt_fixture".into()),
            None,
            HeaderMap::new(),
        )
        .await
        .unwrap();
        stream.pending.clear();
        stream.initial_gap = false;
        streams.push(stream);
    }
    probe.snapshots.store(0, Ordering::Relaxed);
    probe.logs.store(0, Ordering::Relaxed);
    let start = Instant::now();
    for _ in 0..5 {
        for result in
            futures_util::future::join_all(streams.iter_mut().map(Following::wait_for_change)).await
        {
            result.unwrap();
        }
    }
    assert_eq!(start.elapsed(), Duration::from_secs(5));
    assert_eq!(probe.snapshots.load(Ordering::Relaxed), 500);
    assert_eq!(probe.logs.load(Ordering::Relaxed), 0);
    eprintln!("event-wake idle fixture streams=100 duration_s=5 snapshots=500 log_reads=0; transactional Mongo equivalent inferred=300 commands/s");
    drop(f);
}

#[tokio::test(start_paused = true)]
async fn real_router_reconnect_keeps_last_event_id_replay_identical() {
    let f = fixture(true, true).await;
    let app = crate::agent_v2::router(f.state.with_event_wake(true, Duration::from_secs(1)));
    let response = app
        .clone()
        .oneshot(request(
            Method::GET,
            &format!("{EVENTS}?follow=false"),
            "owner-a",
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let wire = String::from_utf8(bytes.to_vec()).unwrap();
    let frames: Vec<_> = wire
        .split("\n\n")
        .filter(|frame| frame.contains("cma.agent.event"))
        .collect();
    assert!(frames.len() >= 2);
    let cursor = frames[0]
        .lines()
        .find_map(|line| line.strip_prefix("id: "))
        .unwrap();
    let mut req = request(
        Method::GET,
        &format!("{EVENTS}?follow=false"),
        "owner-a",
        None,
        None,
    );
    req.headers_mut()
        .insert("last-event-id", cursor.parse().unwrap());
    let resumed = app.oneshot(req).await.unwrap();
    assert_eq!(resumed.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resumed.into_body(), usize::MAX)
        .await
        .unwrap();
    let wire = String::from_utf8(bytes.to_vec()).unwrap();
    assert_eq!(wire.trim_end(), frames[1..].join("\n\n"));
}
