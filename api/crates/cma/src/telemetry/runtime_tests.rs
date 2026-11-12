use super::*;
use std::time::Instant as WallInstant;
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

#[test]
fn pinned_stable_runtime_exposes_worker_busy_time_and_queues() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let metrics = tokio::runtime::Handle::current().metrics();
        assert_eq!(metrics.num_workers(), 2);
        let busy = || {
            (0..metrics.num_workers())
                .map(|worker| metrics.worker_total_busy_duration(worker))
                .sum::<Duration>()
        };
        let before = busy();
        tokio::spawn(async {
            let start = WallInstant::now();
            while start.elapsed() < Duration::from_millis(20) {
                std::hint::black_box(start.elapsed());
            }
        })
        .await
        .unwrap();
        // Busy time is published at scheduler boundaries, not at task return.
        while busy() == before {
            tokio::task::yield_now().await;
        }
        assert!(busy() > before);
        for worker in 0..metrics.num_workers() {
            std::hint::black_box(metrics.worker_park_count(worker));
        }
        std::hint::black_box(metrics.global_queue_depth());
        std::hint::black_box(metrics.num_alive_tasks());
    });
}

struct SequenceProbe {
    snapshots: Mutex<VecDeque<RuntimeSnapshot>>,
    calls: AtomicUsize,
}

impl RuntimeProbe for Arc<SequenceProbe> {
    fn snapshot(&self) -> RuntimeSnapshot {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let mut snapshots = self.snapshots.lock().unwrap();
        if snapshots.len() > 1 {
            snapshots.pop_front().unwrap()
        } else {
            snapshots.front().cloned().unwrap_or_default()
        }
    }
}

fn probe(snapshots: impl IntoIterator<Item = RuntimeSnapshot>) -> Arc<SequenceProbe> {
    Arc::new(SequenceProbe {
        snapshots: Mutex::new(snapshots.into_iter().collect()),
        calls: AtomicUsize::new(0),
    })
}

fn snapshot(workers: &[(u64, u64)]) -> RuntimeSnapshot {
    RuntimeSnapshot {
        workers: workers
            .iter()
            .map(|&(busy_ms, parks)| WorkerSnapshot {
                busy: Duration::from_millis(busy_ms),
                parks,
            })
            .collect(),
        alive_tasks: 7,
        global_queue_depth: 3,
    }
}

#[tokio::test(start_paused = true)]
async fn saturation_requires_every_worker_busy_with_no_parks() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let _recording = metrics::set_default_local_recorder(&recorder);
    let mut sampler = Sampler::new(probe([
        snapshot(&[(0, 0), (0, 0)]),
        snapshot(&[(1_000, 0), (950, 0)]),
        snapshot(&[(2_000, 0), (1_899, 0)]),
        snapshot(&[(3_000, 1), (2_899, 0)]),
        snapshot(&[(4_000, 1), (3_899, 0)]),
    ]));
    sampler.sample(Instant::now());
    for (seconds, busy) in [(1, 2), (1, 1), (1, 2), (2, 2)] {
        tokio::time::advance(INTERVAL).await;
        sampler.sample(Instant::now());
        let text = handle.render();
        assert!(
            text.contains(&format!("cma_runtime_saturated_seconds_total {seconds}")),
            "{text}"
        );
        assert!(
            text.contains(&format!("cma_runtime_busy_workers {busy}")),
            "{text}"
        );
    }
    let text = handle.render();
    assert!(text.contains("cma_runtime_worker_busy_seconds_total{worker=\"1\"} 3"));
    assert!(text.contains("cma_runtime_worker_parks_total{worker=\"0\"} 1"));
    assert!(text.contains("cma_runtime_alive_tasks 7"));
    assert!(text.contains("cma_runtime_global_queue_depth 3"));
}

#[tokio::test(start_paused = true)]
async fn delayed_wake_records_lag_and_skips_missed_ticks_without_false_saturation() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let _recording = metrics::set_default_local_recorder(&recorder);
    let probe = probe([
        snapshot(&[(0, 0), (0, 0)]),
        snapshot(&[(1_000, 0), (1_000, 0)]),
        snapshot(&[(1_500, 0), (1_500, 0)]),
    ]);
    let task = tokio::spawn(Sampler::new(probe.clone()).run());
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(3_500)).await;
    tokio::task::yield_now().await;
    assert_eq!(probe.calls.load(Ordering::Relaxed), 2);
    let text = handle.render();
    assert!(
        text.contains("cma_runtime_wake_lag_seconds_sum 2.5"),
        "{text}"
    );
    assert!(text.contains("cma_runtime_wake_lag_seconds_count 1"));
    assert!(text.contains("cma_runtime_saturated_seconds_total 0"));
    assert!(text.contains("cma_runtime_busy_workers 0"));
    tokio::time::advance(Duration::from_millis(500)).await;
    tokio::task::yield_now().await;
    assert_eq!(probe.calls.load(Ordering::Relaxed), 3);
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    tokio::time::advance(Duration::from_secs(10)).await;
    assert_eq!(probe.calls.load(Ordering::Relaxed), 3);
}

#[tokio::test(start_paused = true)]
async fn zero_workers_topology_changes_and_counter_resets_cannot_create_saturation() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let _recording = metrics::set_default_local_recorder(&recorder);
    let mut sampler = Sampler::new(probe([
        snapshot(&[]),
        snapshot(&[]),
        snapshot(&[(1_000, 5)]),
        snapshot(&[(2_000, 5), (2_000, 5)]),
        snapshot(&[(0, 0), (0, 0)]),
    ]));
    for _ in 0..5 {
        sampler.sample(Instant::now());
        tokio::time::advance(INTERVAL).await;
    }
    let text = handle.render();
    assert!(text.contains("cma_runtime_saturated_seconds_total 0"));
    assert!(text.contains("cma_runtime_busy_workers 0"));
    // Absolute export remains monotonic even if a replacement probe resets.
    assert!(text.contains("cma_runtime_worker_busy_seconds_total{worker=\"0\"} 2"));
    assert!(text.contains("cma_runtime_worker_parks_total{worker=\"0\"} 5"));
}

#[tokio::test(start_paused = true)]
async fn overflow_aggregates_workers_with_at_most_sixty_four_label_values() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let _recording = metrics::set_default_local_recorder(&recorder);
    let mut sampler = Sampler::new(probe([
        snapshot(&[(0, 0); 100]),
        snapshot(&[(2_000, 3); 100]),
    ]));
    sampler.sample(Instant::now());
    tokio::time::advance(Duration::from_secs(2)).await;
    sampler.sample(Instant::now());
    let text = handle.render();
    assert_eq!(
        text.lines()
            .filter(|row| row.starts_with("cma_runtime_worker_busy_seconds_total{"))
            .count(),
        64
    );
    assert!(text.contains("cma_runtime_worker_busy_seconds_total{worker=\"overflow\"} 74"));
    assert!(text.contains("cma_runtime_worker_parks_total{worker=\"overflow\"} 111"));
    assert!(text.contains("cma_runtime_busy_workers 100"));
    assert!(text.contains("cma_runtime_saturated_seconds_total 0"));
    for private in ["agent", "sandbox", "request", "user", "worker=\"63\""] {
        assert!(!text.contains(private), "{text}");
    }
}

#[test]
fn measure_stable_runtime_sampling_cost() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(8)
        .build()
        .unwrap();
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    metrics::with_local_recorder(&recorder, || {
        let probe = TokioProbe(runtime.metrics());
        let iterations = 10_000;
        let start = WallInstant::now();
        for _ in 0..iterations {
            std::hint::black_box(probe.snapshot());
        }
        let read_ns = start.elapsed().as_nanos() / iterations;
        let mut sampler = Sampler::new(probe);
        let start = WallInstant::now();
        for _ in 0..iterations {
            sampler.sample(Instant::now());
        }
        let sample_ns = start.elapsed().as_nanos() / iterations;
        println!("runtime_sampler workers=8 iterations={iterations} snapshot_ns={read_ns} sample_and_export_ns={sample_ns} mongo_commands=0");
    });
}

#[test]
fn fixed_buckets_and_worker_labels_enforce_the_approved_series_cap() {
    for workers in [0_usize, 8, 64, 100] {
        // A broader default must not silently expand these two families.
        let builder = PrometheusBuilder::new()
            .set_buckets(&[0.001, 0.002])
            .unwrap();
        let recorder = configure(builder).unwrap().build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            let mut sampler = Sampler::new(probe([snapshot(&vec![(1_000, 1); workers])]));
            sampler.sample(Instant::now());
            metrics::histogram!("cma_runtime_wake_lag_seconds").record(0.0);
            for pool in ["application", "reporting", "routing"] {
                for state in ["below_limit", "at_limit", "unknown"] {
                    metrics::histogram!("cma_mongo_checkout_wait_seconds", "pool" => pool, "pool_state" => state).record(0.1);
                }
            }
        });
        let text = handle.render();
        let series = text.lines().filter(|row| row.starts_with("cma_")).count();
        assert_eq!(series, 2 * workers.min(64) + 195, "{text}");
        assert!(text.contains("cma_runtime_wake_lag_seconds_bucket{le=\"0.005\"}"));
        assert!(!text.contains("le=\"0.001\""));
    }
}
