use super::*;
use axum::http::StatusCode;
use std::time::Instant;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn personal_agent_list_one_hundred_concurrent_public_readers_meet_budgets() {
    readers("/api/v1/agents", false, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn personal_agent_provenance_one_hundred_concurrent_public_readers_meet_budgets() {
    readers("/api/v1/agents/provenance?limit=200", true, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires disposable MongoDB replica set with profiling"]
async fn personal_agent_list_phase_profile() {
    readers("/api/v1/agents", false, true).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires disposable MongoDB replica set with profiling"]
async fn personal_agent_provenance_phase_profile() {
    readers("/api/v1/agents/provenance?limit=200", true, true).await;
}

async fn readers(path: &'static str, provenance: bool, profiling: bool) {
    let route = if provenance { "provenance" } else { "agents" };
    let subject = if provenance { "provenance" } else { "list" };
    let variant = if profiling {
        "phase_profile"
    } else {
        "one_hundred_concurrent_public_readers_meet_budgets"
    };
    let case = format!("personal_inventory::concurrency::personal_agent_{subject}_{variant}");
    let mut percentiles = Vec::new();
    for distinct in [false, true] {
        let mut record = qualification::Record::new(
            &case,
            if distinct { "distinct" } else { "shared" },
            serde_json::json!({"kind":"readers", "route":route, "mode":"batch", "distinct":distinct,
                "instrumentation":if profiling { "phase-and-mongo-profiler" } else { "command-observer" },
                "expected_readers":100, "expected_warmup":100}),
        );
        let f = if profiling {
            fixture::Fixture::readers_without_command_observer(100, 201, distinct).await
        } else {
            fixture::Fixture::readers(100, 201, distinct).await
        };
        let app = f.app(true, false);
        let mut expected_pages = Vec::new();
        for reader in 0..100 {
            let cookie = identity::cookie(reader);
            let expected = public::request_cookie(&app, path, &cookie).await;
            assert_eq!(expected.0, StatusCode::OK, "reader {reader}");
            assert_eq!(expected.1["items"].as_array().unwrap().len(), 200);
            expected_pages.push((cookie, expected));
            record.set("warmup_completed", reader + 1);
        }
        if profiling {
            profile::server(&f, true).await;
        }
        f.meter.take();
        let barrier = Arc::new(tokio::sync::Barrier::new(101));
        let mut readers = tokio::task::JoinSet::new();
        for (reader, (cookie, expected)) in expected_pages.into_iter().enumerate() {
            let (app, barrier) = (app.clone(), barrier.clone());
            readers.spawn(async move {
                barrier.wait().await;
                let start = Instant::now();
                let phases = profile::Profile::default();
                let work = public::request_cookie(&app, path, &cookie);
                let result = if profiling {
                    use tracing::instrument::WithSubscriber;
                    work.with_subscriber(phases.dispatch()).await
                } else {
                    work.await
                };
                let semantic_match = result == expected;
                (
                    qualification::Sample {
                        reader,
                        status: result.0.as_u16(),
                        elapsed_us: start.elapsed().as_micros(),
                        deadline: false,
                        semantic_match,
                    },
                    phases.take(),
                )
            });
        }
        barrier.wait().await;
        let mut times = Vec::new();
        let mut phases = Vec::new();
        while let Some(result) = readers.join_next().await {
            let (sample, phase) = result.unwrap();
            let valid = sample.status == 200 && sample.semantic_match;
            times.push(sample.elapsed_us);
            record.sample(sample);
            assert!(valid, "concurrent page differs from its owner's reference");
            phases.push(phase);
        }
        times.sort_unstable();
        let mut stats = f.meter.take();
        if profiling {
            profile::server(&f, false).await;
            println!("AGENT_LIST_PROFILE path={path} distinct={distinct}");
            profile::report(&phases);
            let server = profile::server_report(&f).await;
            stats.commands = server.commands;
            stats.failed = server.failed;
        }
        println!(
            "AGENT_LIST_CONCURRENT path={path} readers=100 limit=200 distinct={distinct} p95_us={} p99_us={} stats={stats:?}",
            times[94], times[98]
        );
        record.set("commands", &stats.commands);
        record.set("failed_commands", stats.failed);
        // Every successful request requires exactly this complete set of commands;
        // any retry or getMore makes the total exceed the asserted eight or nine per read.
        let mut expected_commands = std::collections::BTreeMap::from([
            ("agents/aggregate".into(), 100),
            ("agents/find".into(), 100),
            ("auth_browser_logins/find".into(), 200),
            ("auth_subjects/find".into(), 200),
            ("request_rate_limits/findAndModify".into(), 100),
            ("activity/insert".into(), 100),
        ]);
        if provenance {
            expected_commands.remove("agents/find");
            expected_commands.insert("agents/aggregate".into(), 300);
        }
        assert_eq!(stats.commands, expected_commands);
        assert_eq!(stats.failed, 0);
        assert_eq!(stats.count(), stats.checkouts);
        percentiles.push((distinct, times[94], times[98]));
        f.close().await;
        record.complete(true);
    }
    assert!(
        percentiles.iter().all(|(_, p95, _)| *p95 <= 500_000),
        "Agent200 p95 must be at most 500 ms: {percentiles:?}"
    );
    assert!(
        percentiles.iter().all(|(_, _, p99)| *p99 < 2_000_000),
        "Agent200 p99 must be below 2 s: {percentiles:?}"
    );
}
