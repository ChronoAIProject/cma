use super::*;
use axum::{extract::Request, http::HeaderValue, middleware::Next, response::Response};
use futures_util::future::join_all;
use serde_json::{json, Value};
use std::{collections::BTreeMap, sync::Mutex};

tokio::task_local! { static PHASE: &'static str; }

pub(super) async fn foreground(request: Request, next: Next) -> Response {
    let started = Instant::now();
    let mut response = PHASE.scope("foreground", next.run(request)).await;
    response.headers_mut().insert(
        "x-fixture-handler-us",
        HeaderValue::from_str(&started.elapsed().as_micros().to_string()).unwrap(),
    );
    response
}

#[derive(Default)]
struct WaitSamples {
    successful_ms: Vec<f64>,
    failed: usize,
}

#[derive(Default)]
struct TimingSamples {
    waits: BTreeMap<&'static str, WaitSamples>,
    commands: BTreeMap<(&'static str, String), WaitSamples>,
}

#[derive(Clone, Default)]
pub(super) struct TimingMeter(Arc<Mutex<TimingSamples>>);

impl TimingMeter {
    pub(super) fn record(&self, duration: Duration, failed: bool) {
        let phase = PHASE.try_with(|phase| *phase).unwrap_or("unattributed");
        let mut phases = self.0.lock().unwrap();
        let samples = phases.waits.entry(phase).or_default();
        if failed {
            samples.failed += 1;
        } else {
            samples.successful_ms.push(duration.as_secs_f64() * 1000.);
        }
    }

    pub(super) fn command(&self, name: &str, duration: Duration, failed: bool) {
        let phase = PHASE.try_with(|phase| *phase).unwrap_or("unattributed");
        let mut phases = self.0.lock().unwrap();
        let samples = phases.commands.entry((phase, name.to_owned())).or_default();
        if failed {
            samples.failed += 1;
        } else {
            samples.successful_ms.push(duration.as_secs_f64() * 1000.);
        }
    }

    fn take(&self) -> TimingSamples {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

fn distribution(mut samples: Vec<f64>) -> Value {
    samples.sort_by(f64::total_cmp);
    // Cumulative bounds include submillisecond checkout samples and HTTP tails.
    let bounds = [
        0.1, 0.25, 0.5, 1., 2., 5., 10., 20., 30., 50., 100., 250., 1000.,
    ];
    let counts: Vec<_> = bounds
        .iter()
        .map(|bound| samples.partition_point(|sample| sample <= bound))
        .collect();
    json!({
        "count": samples.len(), "bounds_ms": bounds, "cumulative_counts": counts,
        "p95_ms": samples.get(samples.len().saturating_mul(95).div_ceil(100).saturating_sub(1)),
        "max_ms": samples.last(),
        "sum_ms": samples.iter().sum::<f64>(),
    })
}

mod load;
use load::HttpRead;
pub(super) use load::LoadGenerator;

pub(super) async fn measure_window(
    meter: &Meter,
    timings: &TimingMeter,
    managers: &[Manager],
    load: &LoadGenerator,
    defer: bool,
    foreground: bool,
    phase_ms: u64,
) {
    timings.take();
    let before = meter.checkouts.load(Ordering::SeqCst);
    let cpu = cpu_seconds();
    let started = Instant::now();
    let background = async {
        let mut sweep_windows = Vec::new();
        for index in 0..8 {
            tokio::time::sleep_until((started + Duration::from_secs(index)).into()).await;
            if index % 3 == 0 {
                PHASE
                    .scope("maintenance", async {
                        for manager in managers {
                            manager.coordinator.maintain().await.unwrap();
                        }
                    })
                    .await;
            }
            let sweep_start = started.elapsed();
            let slices = PHASE
                .scope(
                    "reconciliation",
                    join_all(managers.iter().map(|manager| {
                        let worker = if defer {
                            &manager.deferred
                        } else {
                            &manager.baseline
                        };
                        async move {
                            let mut sweep = ConnectionSweep::default();
                            let mut slices = 0;
                            loop {
                                slices += 1;
                                assert!(slices <= 50, "reconciliation must complete the cohort");
                                if worker
                                    .reconcile_connection_slice(SystemTime::now(), &mut sweep)
                                    .await
                                    .unwrap()
                                {
                                    break;
                                }
                            }
                            slices
                        }
                    })),
                )
                .await;
            sweep_windows.push((sweep_start, started.elapsed()));
            assert!(
                slices.iter().all(|result| *result >= 2),
                "the live population must span pages"
            );
        }
        sweep_windows
    };
    let reads = load.window(started, foreground, phase_ms);
    let (sweep_windows, (reads, statuses)) = tokio::join!(background, reads);
    tokio::time::sleep_until((started + Duration::from_secs(8)).into()).await;
    let elapsed = started.elapsed().as_secs_f64();
    let checkouts = meter.checkouts.load(Ordering::SeqCst) - before;
    let consumed_cpu = cpu_seconds() - cpu;
    let mut overlap = Vec::new();
    let mut between = Vec::new();
    let mut by_phase: BTreeMap<u64, Vec<f64>> = BTreeMap::new();
    let mut handler_by_phase: BTreeMap<u64, Vec<f64>> = BTreeMap::new();
    let mut dispatch_by_phase: BTreeMap<u64, Vec<f64>> = BTreeMap::new();
    for read in &reads {
        let latency = read.latency_ms();
        if sweep_windows
            .iter()
            .any(|(start, end)| read.start < *end && read.end > *start)
        {
            overlap.push(latency);
        } else {
            between.push(latency);
        }
        by_phase
            .entry(read.scheduled_ms % 1000)
            .or_default()
            .push(latency);
        handler_by_phase
            .entry(read.scheduled_ms % 1000)
            .or_default()
            .push(read.handler_ms);
        dispatch_by_phase
            .entry(read.scheduled_ms % 1000)
            .or_default()
            .push(read.dispatch_delay_ms());
    }
    let http = distribution(reads.iter().map(HttpRead::latency_ms).collect());
    let samples = timings.take();
    let mongo: BTreeMap<_, _> = samples
        .waits
        .into_iter()
        .map(|(phase, samples)| {
            (
                phase,
                json!({"wait": distribution(samples.successful_ms), "failed": samples.failed}),
            )
        })
        .collect();
    let commands: BTreeMap<_, _> = samples.commands.into_iter().map(|((phase, name), samples)| {
        (format!("{phase}/{name}"), json!({"round_trip": distribution(samples.successful_ms), "failed": samples.failed}))
    }).collect();
    let timing_by_phase: BTreeMap<_, _> = by_phase
        .into_iter()
        .map(|(phase, samples)| (phase, distribution(samples)))
        .collect();
    println!(
        "MATCHED_SCHEDULER {}",
        json!({
            "replicas": managers.len(), "defer": defer, "foreground": foreground,
            "http_phase_ms": phase_ms, "sweeps": sweep_windows.len() * managers.len(),
            "foreground_reads": reads.len(), "elapsed_seconds": elapsed,
            "checkouts": checkouts, "checkouts_per_second": checkouts as f64 / elapsed,
            "mean_cores": consumed_cpu / elapsed, "http_p95_ms": http["p95_ms"],
            "http_status_counts": statuses, "rss_kib": rss_kib(), "http_latency": http,
            "scheduled_to_send_ms": distribution(reads.iter().map(HttpRead::dispatch_delay_ms).collect()),
            "handler_duration_ms": distribution(reads.iter().map(|read| read.handler_ms).collect()),
            "mongo_command_round_trip_by_phase": commands,
            "http_overlapping_sweep": distribution(overlap), "http_between_sweeps": distribution(between),
            "http_by_scheduled_phase_ms": timing_by_phase, "mongo_checkout_by_phase": mongo,
            "handler_by_scheduled_phase_ms": handler_by_phase.into_iter().map(|(phase, samples)| (phase, distribution(samples))).collect::<BTreeMap<_, _>>(),
            "dispatch_by_scheduled_phase_ms": dispatch_by_phase.into_iter().map(|(phase, samples)| (phase, distribution(samples))).collect::<BTreeMap<_, _>>(),
            "sweep_duration_ms": distribution(sweep_windows.iter().map(|(start, end)| (*end - *start).as_secs_f64() * 1000.).collect()),
        })
    );
}
