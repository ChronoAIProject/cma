#[path = "latency/app.rs"]
mod app;
#[path = "latency/batch_gate.rs"]
mod batch_gate;
#[path = "latency/batch_tests.rs"]
mod batch_tests;
#[path = "fake_cma.rs"]
pub mod fake_cma;
#[path = "fake_provider.rs"]
pub mod fake_provider;
#[path = "latency/identity.rs"]
mod identity;
#[path = "relay/support.rs"]
pub mod relay_support;
#[path = "relay/signer.rs"]
pub mod signer;

use app::{App, LANES};
use futures_util::{stream, StreamExt};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "executes the warm/cold/socket and paced tool delivery budgets"]
async fn latency_budget() {
    run(App::new().await, "cmaeg-latency.txt").await;
}

/// The same budgets with `CMAEG_CONSOLE_ENABLED=true` and a warm owner
/// index: every hot path reports to the console's observer, which must
/// never make it wait or drop an observation (rollout step R1).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "executes the latency budgets with the console's observers composed"]
async fn latency_budget_with_console() {
    run(App::with_console().await, "cmaeg-latency-console.txt").await;
}

async fn run(mut app: App, report_file: &str) {
    cmaeg::init_logging();
    let metrics = cmaeg::telemetry::install().unwrap();
    measure(app.callback("prime", 0).await, 202).await;
    app.quiet().await;
    let before_warm = app.cma.gets.load(Ordering::SeqCst);
    let warm: Vec<_> = stream::iter(0..500)
        .map(|index| {
            let app = &app;
            async move {
                measure(
                    app.callback(&format!("warm-{index}"), index % LANES).await,
                    202,
                )
                .await
            }
        })
        .buffer_unordered(16)
        .collect()
        .await;
    app.quiet().await;
    assert_eq!(
        app.cma.gets.load(Ordering::SeqCst),
        before_warm,
        "warm ingress called CMA"
    );
    assert_eq!(
        app.identity.jwks_calls.load(Ordering::SeqCst),
        1,
        "warm ingress fetched JWKS"
    );

    let mut cold = Vec::new();
    for batch in 0..25 {
        // Build and sign the full batch before invalidating the warm channel.
        // The eight simultaneous cold callers share the production restore flight.
        let requests =
            app.cold_callbacks((0..8).map(|lane| (format!("cold-{batch}-{lane}"), lane)));
        let pending = requests
            .into_iter()
            .map(|request| async move { measure(request.send().await.unwrap(), 202).await });
        cold.extend(futures_util::future::join_all(pending).await);
    }
    app.quiet().await;
    assert_eq!(app.cma.gets.load(Ordering::SeqCst) - before_warm, 25);
    assert_eq!(app.identity.jwks_calls.load(Ordering::SeqCst), 1);
    assert!(
        cold.iter().all(|sample| sample["restore"] > 0.5),
        "cold run contained a warm cache hit"
    );

    let references: BTreeMap<_, _> = app
        .provider
        .requests()
        .into_iter()
        .filter(|request| request.method == "POST")
        .map(|request| {
            let body: Value = serde_json::from_str(&request.body).unwrap();
            (
                body["event_context"]["activity"]["event_id"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
                body["event_context"]["event_ref"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
            )
        })
        .collect();
    assert_eq!(references.len(), 701);
    let mut tools = Vec::new();
    let mut next_round = Instant::now();
    for start in (0..200).step_by(LANES) {
        tokio::time::sleep_until(next_round.into()).await;
        let pending = (start..(start + LANES).min(200)).map(|index| {
            let app = &app;
            let reference = &references[&format!("warm-{}", index % LANES)];
            async move {
                let response = app
                    .client
                    .post(format!("{}/v1/events/{reference}/replies", app.base))
                    .bearer_auth(&app.agent_key)
                    .json(&json!({"text":format!("tool-{index}")}))
                    .send()
                    .await
                    .unwrap();
                measure(response, 200).await
            }
        });
        tools.extend(futures_util::future::join_all(pending).await);
        // Start each route's next request after the previous receipt + the full
        // production interval; no mocked clock or altered pacing implementation.
        next_round = Instant::now() + Duration::from_secs(3);
    }
    assert_eq!(app.identity.replies.lock().unwrap().len(), 200);
    let mut sent = BTreeMap::new();
    for (route, at) in app.identity.replies.lock().unwrap().iter() {
        if let Some(previous) = sent.insert(route, *at) {
            assert!(at.duration_since(previous) >= Duration::from_secs(3));
        }
    }
    app.drain().await;

    let runs = [
        ("warm launch", values(&warm, "launch"), 1000.0),
        ("cold launch", values(&cold, "launch"), 2000.0),
        ("cold restore", values(&cold, "restore"), 800.0),
        ("tool deliver", values(&tools, "deliver"), 500.0),
    ];
    let mut report = format!("CMAEG latency budget; console={}; milliseconds; fake CMA delay={} ms\n{:<16} {:>6} {:>10} {:>10} {:>10} {:>10}\n", app.console.is_some(), app.restore_delay, "measurement", "count", "p50", "p90", "p99", "p99 limit");
    for (name, values, budget) in &runs {
        report.push_str(&format!(
            "{name:<16} {:>6} {:>10.3} {:>10.3} {:>10.3} {budget:>10.0}\n",
            values.len(),
            percentile(values, 50),
            percentile(values, 90),
            percentile(values, 99)
        ));
    }
    let target = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
    std::fs::create_dir_all(&target).unwrap();
    if let Some(console) = &app.console {
        // Every admitted event reached the console's memory, and none was
        // dropped on the way.
        assert_eq!(console.observer.dropped(), 0, "observations were dropped");
        let totals = console.memory.activity_totals();
        report.push_str(&format!(
            "console: observer drops 0, activity entries {}, activity bytes {}\n",
            totals.entries, totals.bytes
        ));
    }
    std::fs::write(target.join(report_file), &report).unwrap();
    println!("{report}");
    assert!(
        percentile(&runs[0].1, 50) < 50.0,
        "warm median exceeded 50 ms\n{report}"
    );
    for (name, samples, budget) in runs {
        assert!(
            percentile(&samples, 99) <= budget,
            "{name} exceeded budget\n{report}"
        );
    }
    let server =
        cmaeg::http::metrics::MetricsServer::start("127.0.0.1:0".parse().unwrap(), metrics)
            .await
            .unwrap();
    let output = tokio::process::Command::new("curl")
        .args([
            "--silent",
            "--show-error",
            "--fail",
            "--max-time",
            "5",
            &format!("http://{}/metrics", server.address()),
        ])
        .output()
        .await
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    std::fs::write(target.join("cmaeg-metrics.txt"), &text).unwrap();
    for name in cmaeg::telemetry::names::HISTOGRAMS {
        assert!(text.contains(name));
    }
    assert!(text.contains("cold=\"true\""));
    assert!(text.contains("cmaeg_turns_inflight 0"));
    assert!(text.contains("cmaeg_channels_cached 1"));
    assert!(text
        .lines()
        .any(|line| line.starts_with("cmaeg_delivery_seconds_count{")
            && line.contains("telegram")
            && line.ends_with(" 200")));
}
async fn measure(response: reqwest::Response, status: u16) -> BTreeMap<String, f64> {
    let code = response.status().as_u16();
    let header = response
        .headers()
        .get("server-timing")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let body: Value = response.json().await.unwrap();
    assert_eq!(code, status, "{body}");
    assert!(!header.contains("not_written"), "{header}");
    if status == 202 {
        assert_eq!(body["status"], "accepted");
    } else {
        assert_eq!(body["delivery"]["outcome"], "confirmed");
    }
    let values: BTreeMap<String, f64> = header
        .split(',')
        .map(|part| {
            let (name, value) = part.trim().split_once(";dur=").unwrap();
            (
                name.into(),
                value.split(';').next().unwrap().parse().unwrap(),
            )
        })
        .collect();
    assert!(values
        .values()
        .all(|value| value.is_finite() && *value >= 0.0));
    if let Some(total) = values.get("total") {
        for name in ["restore", "verify", "launch"] {
            assert!(*total >= values[name]);
        }
        assert!(values["launch"] > 0.0);
    }
    values
}
fn values(samples: &[BTreeMap<String, f64>], field: &str) -> Vec<f64> {
    let mut values: Vec<_> = samples.iter().map(|sample| sample[field]).collect();
    values.sort_by(f64::total_cmp);
    values
}
fn percentile(values: &[f64], percent: usize) -> f64 {
    values[(values.len() * percent).div_ceil(100) - 1]
}
