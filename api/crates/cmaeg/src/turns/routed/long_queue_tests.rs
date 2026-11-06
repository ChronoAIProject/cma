use super::{
    adapter_test_server::HttpServer,
    delivery_tests::stored,
    executor_tests::executor_with_timeout,
    scheduler::{Disposition, TargetExecutor},
    test_support::*,
    *,
};
use cmaeg_core::channel::ProviderTarget;
use serde_json::{json, Value};
use std::{sync::atomic::Ordering, time::Duration};

fn response(status: &str) -> Value {
    json!({"id":"resp_accepted","object":"response","status":status,
        "conversation":{"id":"agt_native"},"metadata":{},"output":[]})
}

fn lifecycle(sequence: u64, kind: &str, status: &str) -> Value {
    json!({"type":kind,"sequence_number":sequence,"response":response(status)})
}

async fn advance_elapsed(setup: &Setup, seconds: u64) {
    // No executor owns a lease between invocations. Advance both persisted
    // wall-clock deadlines and Tokio timers, then resume real loopback IO.
    tokio::time::pause();
    setup
        .server
        .memory
        .clock
        .fetch_add(seconds as i64, Ordering::SeqCst);
    tokio::time::advance(Duration::from_secs(seconds)).await;
    tokio::time::resume();
}

async fn recoverable(setup: &Setup) {
    let store = setup.server.store();
    let mut cursor = None;
    loop {
        let page = store.recover_all_page(cursor.as_deref(), 32).await.unwrap();
        assert!(page.issues.is_empty());
        if page
            .plans
            .iter()
            .any(|plan| plan.admission_id == setup.plan.admission_id)
        {
            return;
        }
        cursor = Some(
            page.next_cursor
                .expect("the admitted manifest remains discoverable"),
        );
    }
}

#[tokio::test]
async fn routed_existing_agent_queued_nearly_a_day_then_runs_thirty_minutes_completes_once() {
    const QUEUE_POLICY: u32 = 86400;
    const QUEUED_FOR: u64 = 86400 - 60;
    let request = fixture::request(
        |value| {
            value["provider"]["target"] = json!({"type":"existing_agent","agent_id":"agt_native"});
            value["profile"]["queue_lifetime_seconds"] = QUEUE_POLICY.into();
            value["profile_digest"] =
                crate::channels::routing_fixture::digest_of(&value["profile"]).into();
            value["routing"]["consumers"] = json!([]);
            value["secrets"]["consumer_keys"] = json!([]);
            value["reply"]["sinks"] = json!([]);
            value["reply"]["auto_final"] = true.into();
        },
        fixture::custom(None),
    );
    assert!(matches!(
        request.channel.definition.provider.target,
        ProviderTarget::ExistingAgent { .. }
    ));
    assert_eq!(
        request.channel.definition.profile.queue_lifetime_seconds,
        Some(QUEUE_POLICY)
    );
    let setup = Setup::with_request(supported(), request).await;
    assert!(!setup.plan.prepares_binding(setup.target()));
    let provider = HttpServer::start().await;
    {
        let mut memory = provider.memory.lock().unwrap();
        memory.config.snapshot_body = Some(response("queued"));
        memory.config.replay_frames = Some(vec![lifecycle(0, "response.created", "queued")]);
    }
    let (runner, mut outbound, work) =
        executor_with_timeout(&setup, &provider, Duration::from_secs(30));
    let began = tokio::time::Instant::now();
    let mut elapsed = 0;
    for age in [0, 301, 601, 1801, 7200, QUEUED_FOR] {
        advance_elapsed(&setup, age - elapsed).await;
        elapsed = age;
        recoverable(&setup).await;
        assert!(
            matches!(runner.execute(&work).await, Disposition::Retry(_)),
            "queued response at age {age}s must remain recoverable"
        );
        assert!(matches!(stored(&setup).await.phase, Phase::Accepted {
            ref response_id, durable_cursor: Some(0)
        } if response_id == "resp_accepted"));
        assert_eq!(
            provider
                .seen()
                .iter()
                .filter(|request| request.method == "POST")
                .count(),
            1
        );
        assert!(outbound.try_recv().is_err());
    }

    // Recover with a fresh executor, as after a process restart. The same CMA
    // response begins running shortly before its queue deadline.
    let (restarted, mut recovered_outbound, work) =
        executor_with_timeout(&setup, &provider, Duration::from_secs(30));
    {
        let mut memory = provider.memory.lock().unwrap();
        memory.config.snapshot_body = Some(response("in_progress"));
        memory.config.replay_frames = Some(vec![
            lifecycle(0, "response.created", "queued"),
            lifecycle(1, "response.in_progress", "in_progress"),
        ]);
    }
    assert!(matches!(
        restarted.execute(&work).await,
        Disposition::Retry(_)
    ));
    advance_elapsed(&setup, 1200).await;
    assert!(matches!(
        restarted.execute(&work).await,
        Disposition::Retry(_)
    ));
    assert!(matches!(
        stored(&setup).await.phase,
        Phase::Accepted {
            durable_cursor: Some(1),
            ..
        }
    ));
    advance_elapsed(&setup, 600).await;
    recoverable(&setup).await;
    {
        let mut memory = provider.memory.lock().unwrap();
        memory.config.snapshot_body = Some(response("completed"));
        memory
            .config
            .replay_frames
            .as_mut()
            .unwrap()
            .push(lifecycle(2, "response.completed", "completed"));
    }
    assert!(matches!(
        restarted.execute(&work).await,
        Disposition::Settled
    ));
    let terminal = stored(&setup).await;
    assert!(matches!(
        terminal.phase,
        Phase::Completed {
            durable_cursor: Some(2),
            ..
        }
    ));
    assert!(terminal.delivery.as_ref().unwrap().terminal);
    assert!(terminal.delivery.as_ref().unwrap().status.settled());
    assert_eq!(
        provider
            .seen()
            .iter()
            .filter(|request| request.method == "POST")
            .count(),
        1
    );
    assert!(recovered_outbound.try_recv().is_err());
    assert!(began.elapsed() >= Duration::from_secs(QUEUED_FOR + 1800));

    // A later scheduler visit is a terminal no-op, not another submission,
    // completion frame, or delivery acknowledgement.
    let requests = provider.seen().len();
    for _ in 0..2 {
        assert!(matches!(
            restarted.execute(&work).await,
            Disposition::Settled
        ));
        let replay = stored(&setup).await;
        assert_eq!(replay.phase, terminal.phase);
        assert_eq!(
            serde_json::to_value(replay.delivery).unwrap(),
            serde_json::to_value(&terminal.delivery).unwrap()
        );
    }
    assert_eq!(provider.seen().len(), requests);
}
