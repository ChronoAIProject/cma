//! Dense immutable history must preserve exact projections across cursor batches.
use super::*;
use layer2_core::sandbox_usage::{UsageEvent, UsageEventKind};
use mongodb::event::{command::CommandEvent, EventHandler};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex,
};
use tracing::instrument::WithSubscriber;

#[derive(Default)]
pub(super) struct Commands {
    pub(super) event_reads: AtomicUsize,
    pub(super) returned: AtomicUsize,
    max_batch: AtomicUsize,
    pending: Mutex<std::collections::HashSet<i32>>,
}

pub(super) async fn observed_store() -> (Arc<ReportingStore>, Arc<Commands>) {
    observe_store(fixture().await).await
}

async fn observe_store(original: Arc<ReportingStore>) -> (Arc<ReportingStore>, Arc<Commands>) {
    let commands = Arc::new(Commands::default());
    let observed = commands.clone();
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    configure_pool(&mut options, 4);
    options.command_event_handler = Some(EventHandler::callback(move |event| match event {
        CommandEvent::Started(e)
            if e.command.get_str("find").ok() == Some("sandbox_usage_events")
                || e.command.get_str("collection").ok() == Some("sandbox_usage_events") =>
        {
            observed.event_reads.fetch_add(1, Ordering::SeqCst);
            observed.pending.lock().unwrap().insert(e.request_id);
        }
        CommandEvent::Succeeded(e) if observed.pending.lock().unwrap().remove(&e.request_id) => {
            let cursor = e.reply.get_document("cursor").unwrap();
            let batch = cursor
                .get_array("firstBatch")
                .or_else(|_| cursor.get_array("nextBatch"))
                .unwrap();
            observed.returned.fetch_add(batch.len(), Ordering::SeqCst);
            observed.max_batch.fetch_max(batch.len(), Ordering::SeqCst);
        }
        _ => {}
    }));
    let client = Client::with_options(options).unwrap();
    let mut store = (*original).clone();
    store.persistence = store.persistence.with_client(client);
    (Arc::new(store), commands)
}

pub(super) fn event(state: &SandboxUsage, n: u64, start: u64) -> UsageEvent {
    let allocation = state.allocation.clone();
    UsageEvent {
        sandbox_id: state.sandbox_id.clone(),
        owner: state.owner.clone(),
        revision: n + 1,
        source_seq: n + 1,
        kind: UsageEventKind::Checkpoint,
        started_at: start,
        ended_at: start + 60,
        quantity: allocation.usage(60).unwrap(),
        next_allocation: allocation.clone(),
        allocation,
        status: "active".into(),
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn dense_history_batches_preserve_exact_allocation_with_fewer_round_trips() {
    let (store, commands) = observed_store().await;
    let day = (now() / 86_400 - 2) * 86_400;
    let range = Range {
        from: day + 60,
        to: day + 60 * 241,
    };
    // 48,000 retained events is below the existing per-page/day 100k bound.
    // It requires 240 old 200-document batches, versus 24 batches at 2,000.
    let mut states = Vec::new();
    for id in 0..200 {
        let mut s = state(id, range.from);
        s.last_observed_at = range.to;
        s.revision = 240;
        let docs = (0..240)
            .map(|n| {
                super::super::super::sandbox_usage::event_document(&event(
                    &s,
                    n,
                    range.from + n * 60,
                ))
                .unwrap()
            })
            .collect::<Vec<_>>();
        store
            .collection("sandbox_usage_events")
            .insert_many(docs)
            .await
            .unwrap();
        states.push(s);
    }
    store
        .collection("sandbox_usage_events")
        .create_index(
            mongodb::IndexModel::builder()
                .keys(doc! {"sandbox_id":1,"revision":1})
                .build(),
        )
        .await
        .unwrap();
    let ids = states
        .iter()
        .map(|s| s.sandbox_id.as_str())
        .collect::<Vec<_>>();
    let started = std::time::Instant::now();
    let mut session = store.snapshot().await.unwrap();
    let mut cursor=store.collection("sandbox_usage_events").find(doc!{"sandbox_id":{"$in":&ids},"ended_at":{"$gt":range.from as i64},"started_at":{"$lt":range.to as i64}}).sort(doc!{"sandbox_id":1,"revision":1}).batch_size(200).session(&mut session).await.unwrap();
    let mut baseline = std::collections::BTreeMap::<String, Vec<UsageEvent>>::new();
    while let Some(d) = cursor.next(&mut session).await.transpose().unwrap() {
        let e = super::super::super::sandbox_usage::read_event(&d).unwrap();
        baseline
            .entry(e.sandbox_id.to_string())
            .or_default()
            .push(e);
    }
    let baseline_us = started.elapsed().as_micros();
    let baseline_reads = commands.event_reads.swap(0, Ordering::SeqCst);
    assert_eq!(commands.returned.swap(0, Ordering::SeqCst), 48_000);
    assert_eq!(commands.max_batch.swap(0, Ordering::SeqCst), 200);
    let started = std::time::Instant::now();
    let result = store
        .allocation_periods(&mut store.snapshot().await.unwrap(), &states, range)
        .await
        .unwrap();
    let optimized_us = started.elapsed().as_micros();
    let optimized_reads = commands.event_reads.swap(0, Ordering::SeqCst);
    assert_eq!(commands.returned.swap(0, Ordering::SeqCst), 48_000);
    assert!(commands.max_batch.swap(0, Ordering::SeqCst) <= 2_000);
    assert!(
        optimized_reads * 8 < baseline_reads,
        "{optimized_reads} vs {baseline_reads}"
    );
    for s in &states {
        let expected = allocation::project(s, &baseline[s.sandbox_id.as_str()], range);
        assert_eq!(
            serde_json::to_value(&result[s.sandbox_id.as_str()]).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
        let p = &result[s.sandbox_id.as_str()].period;
        assert!(p.complete);
        assert_eq!(p.uptime_seconds, 14_400.);
        assert!((p.cpu_core_hours - 8.).abs() < 1e-10);
    }
    // Completed exact-day-slice reuse reads no source events.
    let cached = store
        .allocation_periods(&mut store.snapshot().await.unwrap(), &states, range)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(cached).unwrap(),
        serde_json::to_value(result).unwrap()
    );
    assert_eq!(commands.event_reads.load(Ordering::SeqCst), 0);
    eprintln!("history_batch events=48000 baseline_reads={baseline_reads} optimized_reads={optimized_reads} baseline_us={baseline_us} optimized_us={optimized_us}");
    // Query-field corruption in a later batch must still fail closed.
    store
        .collection(super::super::super::sandbox_usage_rollups::COLLECTION)
        .delete_many(doc! {})
        .await
        .unwrap();
    store
        .collection("sandbox_usage_events")
        .update_one(
            doc! {"sandbox_id":states.last().unwrap().sandbox_id.as_str(),"revision":239_i64},
            doc! {"$set":{"owner":"foreign"}},
        )
        .await
        .unwrap();
    assert!(store
        .allocation_periods(&mut store.snapshot().await.unwrap(), &states, range)
        .await
        .is_err());
    store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn seven_day_history_generates_and_reuses_complete_421_member_report() {
    let preparation = std::time::Instant::now();
    // The owned local lane is restricted to two exact disposable databases.
    // Ordinary CI keeps its existing unique per-test database behavior.
    let original = match std::env::var("CHRONO_TEST_ALLOCATION_DB_01") {
        Ok(database) => {
            assert!(database.starts_with("metrics_2140_"));
            ReportingStore::connect(
                &std::env::var("CHRONO_TEST_MONGODB_URI").unwrap(),
                &database,
                &[19; 32],
            )
            .await
            .unwrap()
        }
        Err(std::env::VarError::NotPresent) => fixture().await,
        Err(error) => panic!("invalid named disposable database: {error}"),
    };
    let (store, commands) = observe_store(original).await;
    let from = (now() / 86_400 - 8) * 86_400;
    let to = from + 7 * 86_400;
    const MEMBERS: usize = 421;
    let mut states = Vec::new();
    for id in 0..MEMBERS {
        let mut s = state(id, from);
        s.last_observed_at = to;
        s.revision = 7 * 48;
        let docs = (0..7 * 48)
            .map(|n| {
                let mut e = event(&s, n, from + n * 1800);
                e.ended_at = e.started_at + 1800;
                e.quantity = e.allocation.usage(1800).unwrap();
                super::super::super::sandbox_usage::event_document(&e).unwrap()
            })
            .collect::<Vec<_>>();
        store
            .collection("sandbox_usage_events")
            .insert_many(docs)
            .await
            .unwrap();
        states.push(s);
    }
    seed(&store, &states).await;
    store
        .collection("sandbox_usage_events")
        .create_index(
            mongodb::IndexModel::builder()
                .keys(doc! {"sandbox_id":1,"revision":1})
                .build(),
        )
        .await
        .unwrap();
    let query = ReportInput {
        from: Some(from),
        to: Some(to),
        ..Default::default()
    }
    .validate(now())
    .unwrap();
    let preparation_ms = preparation.elapsed().as_millis();
    for run in 0..2 {
        let principal = format!("operator-{run}");
        let pending = store
            .submit(scope(&principal, true), query.clone(), now())
            .await
            .unwrap();
        let lease = store
            .claim_report("report:0", "history-worker")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(lease.report.report_id, pending.report_id);
        commands.event_reads.store(0, Ordering::SeqCst);
        commands.returned.store(0, Ordering::SeqCst);
        let subscriber = tracing_subscriber::fmt()
            .json()
            .without_time()
            .with_ansi(false)
            .with_current_span(false)
            .with_span_list(false)
            .with_env_filter("off,cma::sandbox_metrics::report_trace=info")
            .with_writer(std::io::stderr)
            .finish();
        let start = std::time::Instant::now();
        let result =
            crate::sandbox_metrics::generate_fixture_with_collection(store.clone(), &lease, false)
                .with_subscriber(subscriber)
                .await;
        eprintln!(
            "history_week_generation run={run} members={MEMBERS} preparation_ms={preparation_ms} generation_ms={} success={} event_reads={} returned_events={}",
            start.elapsed().as_millis(),
            result.is_ok(),
            commands.event_reads.load(Ordering::SeqCst),
            commands.returned.load(Ordering::SeqCst)
        );
        let generated = result.unwrap();
        let summary = generated.summary.as_ref().unwrap();
        assert_eq!(summary.sandbox_count, MEMBERS as u64);
        assert!(summary.allocated_period.complete);
        assert_eq!(
            summary.allocated_period.uptime_seconds,
            MEMBERS as f64 * 7. * 86_400.
        );
        assert_eq!(
            summary.allocated_period.cpu_core_hours,
            MEMBERS as f64 * 7. * 24. * 2.
        );
        let reads = commands.event_reads.load(Ordering::SeqCst);
        if run == 0 {
            assert_eq!(commands.returned.load(Ordering::SeqCst), MEMBERS * 7 * 48);
            // Each 200-member page is split into <=8-member full-range reads.
            // Count both find and getMore, preserving an exact query-shape
            // contract (105 here), rather than the old daily-path bound.
            let expected_reads: usize = (0..MEMBERS)
                .step_by(200)
                .map(|page| {
                    let members = (MEMBERS - page).min(200);
                    (0..members)
                        .step_by(8)
                        .map(|group| ((members - group).min(8) * 7 * 48).div_ceil(2_000))
                        .sum::<usize>()
                })
                .sum();
            assert_eq!(reads, expected_reads);
        } else {
            assert_eq!(reads, 0);
        }
        store.finish_report(&lease, generated, true).await.unwrap();
        let report = store
            .report(&scope(&principal, true), &pending.report_id, now())
            .await
            .unwrap();
        assert_eq!(report.status, ReportStatus::Ready);
        eprintln!(
            "history_week run={run} members={MEMBERS} elapsed_ms={} event_reads={reads}",
            start.elapsed().as_millis()
        );
    }
    store.db.drop().await.unwrap();
}
