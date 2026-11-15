//! Same data/work in ABBA order. Timing is evidence, never a machine-speed gate.
use super::*;
use super::{
    fixture::*,
    observation::{distribution, polls, Observation},
};
use crate::store::execution_sections::MonotonicClock;
use tokio::{sync::Barrier, task::JoinSet};

const WORKERS: usize = 6;
const SCANS_PER_WORKER: usize = 8;
const READ_PAIRS: usize = 200;

#[derive(Default)]
struct Work {
    scans: Vec<Duration>,
    polls: Vec<Duration>,
    execution: Vec<(SectionStats, u64)>,
}

async fn phase(
    client: &Client,
    db: &Database,
    id: &AgentId,
    enabled: bool,
    raw_batches: bool,
    bytes: u64,
) -> serde_json::Value {
    let barrier = Arc::new(Barrier::new(WORKERS + 1));
    let mut tasks = JoinSet::new();
    for _ in 0..WORKERS {
        let (client, collection, id, barrier) = (
            client.clone(),
            db.collection::<Document>("agent_log"),
            id.clone(),
            barrier.clone(),
        );
        tasks.spawn(isolated_test(async move {
            barrier.wait().await;
            let mut work = Work::default();
            for _ in 0..SCANS_PER_WORKER {
                let mut session = client.start_session().await.unwrap();
                let started = Instant::now();
                let ((result, execution), durations) = polls(recorded_execution(scan_collection(
                    ScanCaller::Other,
                    &collection,
                    &id,
                    10_001,
                    &mut session,
                    ScanOptions::<MonotonicClock>::new(false, Policy::enabled(enabled))
                        .with_raw_batches(raw_batches),
                    |_| Ok(()),
                )))
                .await;
                assert!(result.is_ok());
                work.scans.push(started.elapsed());
                work.polls.extend(durations);
                work.execution.extend(execution);
            }
            assert_eq!(
                &snapshot(ScanCaller::Other, true)[..3],
                &[
                    SCANS_PER_WORKER as u64,
                    SCANS_PER_WORKER as u64 * 10_001,
                    SCANS_PER_WORKER as u64 * bytes
                ]
            );
            assert_eq!(snapshot(ScanCaller::Other, false)[0], 0);
            work
        }));
    }
    let points = db.collection::<Document>("point_reads");
    let read_task = tokio::spawn(async move {
        barrier.wait().await;
        let mut times = Vec::new();
        for _ in 0..READ_PAIRS {
            let start = Instant::now();
            for _ in 0..2 {
                assert!(points
                    .find_one(doc! {"_id": "point"})
                    .await
                    .unwrap()
                    .is_some());
            }
            times.push(start.elapsed());
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        times
    });
    let mut total = Work::default();
    while let Some(result) = tasks.join_next().await {
        let work = result.unwrap();
        total.scans.extend(work.scans);
        total.polls.extend(work.polls);
        total.execution.extend(work.execution);
    }
    let reads = read_task.await.unwrap();
    let sections = total
        .execution
        .iter()
        .map(|(stats, _)| stats.longest_section)
        .collect::<Vec<_>>();
    let driver = total
        .execution
        .iter()
        .map(|(stats, _)| stats.longest_poll)
        .collect::<Vec<_>>();
    serde_json::json!({"enabled": enabled, "raw_batches": raw_batches, "scans": total.scans.len(),
        "documents": total.scans.len() * 10_001, "bytes": total.scans.len() as u64 * bytes,
        "point_read_pairs": reads.len(), "scan_latency": distribution(&total.scans),
        "task_polls": distribution(&total.polls), "longest_sections": distribution(&sections),
        "driver_polls": distribution(&driver), "point_pair_latency": distribution(&reads),
        "yields": total.execution.iter().map(|(_, yields)| yields).sum::<u64>()})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn local_scan_scheduling_abba_reproduction() {
    reproduction(false, "scan_scheduling_reproduction").await;
    reproduction(true, "scan_cursor_delivery_reproduction").await;
}

async fn reproduction(cooperative_control: bool, label: &str) {
    let observation = Observation::default();
    let client = observation.client().await;
    let db = database(&client);
    let collection = db.collection::<Document>("agent_log");
    collection
        .create_index(
            mongodb::IndexModel::builder()
                .keys(doc! {"agent_id":1, "seq":1})
                .build(),
        )
        .await
        .unwrap();
    let id = AgentId("agt_scheduling_fixture".into());
    let bytes = insert(&collection, &entries(&id, 10_001, 256)).await;
    db.collection::<Document>("point_reads")
        .insert_one(doc! {"_id": "point"})
        .await
        .unwrap();
    // The scheduling comparison is off/on; the delivery comparison keeps
    // cooperation enabled in both arms and changes only cursor delivery.
    for raw_batches in [false, true] {
        phase(
            &client,
            &db,
            &id,
            cooperative_control || raw_batches,
            raw_batches,
            bytes,
        )
        .await;
    }
    for (round, raw_batches) in [false, true, true, false].into_iter().enumerate() {
        let enabled = cooperative_control || raw_batches;
        observation.begin();
        let start = Instant::now();
        let mut result = phase(&client, &db, &id, enabled, raw_batches, bytes).await;
        let sample = observation.finish();
        result["round"] = round.into();
        result["elapsed_seconds"] = start.elapsed().as_secs_f64().into();
        result["commands"] = serde_json::to_value(&sample.commands).unwrap();
        result["command_round_trips"] = sample.commands.values().sum::<u64>().into();
        result["pool_event_gaps"] = distribution(&sample.gaps);
        result["checkout_wait"] = distribution(&sample.checkout);
        result["pool_max_in_use"] = sample.in_use_max.into();
        result["pool_capacity"] = 30.into();
        assert_eq!(
            sample.commands.get("find"),
            Some(&((WORKERS * SCANS_PER_WORKER + READ_PAIRS * 2) as u64))
        );
        assert_eq!(
            sample.commands.get("getMore"),
            Some(&((WORKERS * SCANS_PER_WORKER) as u64))
        );
        println!("{label} {result}");
    }
    db.drop().await.unwrap();
}
