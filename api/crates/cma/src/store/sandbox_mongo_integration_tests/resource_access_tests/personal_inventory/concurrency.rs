use super::*;
use axum::http::StatusCode;
use std::time::Instant;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn personal_agent_list_one_hundred_concurrent_public_readers_meet_budgets() {
    readers("/api/v1/agents", false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn personal_agent_provenance_one_hundred_concurrent_public_readers_meet_budgets() {
    readers("/api/v1/agents/provenance?limit=200", true).await;
}

async fn readers(path: &'static str, provenance: bool) {
    let mut percentiles = Vec::new();
    for distinct in [false, true] {
        let f = fixture::Fixture::readers(100, 201, distinct).await;
        let app = f.app(true, false);
        let mut expected_pages = Vec::new();
        for reader in 0..100 {
            let cookie = identity::cookie(reader);
            let expected = public::request_cookie(&app, path, &cookie).await;
            assert_eq!(expected.0, StatusCode::OK, "reader {reader}");
            assert_eq!(expected.1["items"].as_array().unwrap().len(), 200);
            expected_pages.push((cookie, expected));
        }
        f.meter.take();
        let barrier = Arc::new(tokio::sync::Barrier::new(101));
        let mut readers = tokio::task::JoinSet::new();
        for (cookie, expected) in expected_pages {
            let (app, barrier) = (app.clone(), barrier.clone());
            readers.spawn(async move {
                barrier.wait().await;
                let start = Instant::now();
                let result = public::request_cookie(&app, path, &cookie).await;
                assert_eq!(
                    result.0,
                    expected.0,
                    "{}",
                    result.1.get("code").unwrap_or(&serde_json::Value::Null)
                );
                assert!(
                    result.1 == expected.1,
                    "concurrent page differs from its owner's reference"
                );
                start.elapsed().as_micros()
            });
        }
        barrier.wait().await;
        let mut times = Vec::new();
        while let Some(result) = readers.join_next().await {
            times.push(result.unwrap());
        }
        times.sort_unstable();
        let stats = f.meter.take();
        println!("AGENT_LIST_CONCURRENT path={path} readers=100 limit=200 distinct={distinct} p95_us={} p99_us={} stats={stats:?}", times[94], times[98]);
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
        percentiles.push((distinct, times[94], times[98]));
        f.close().await;
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
