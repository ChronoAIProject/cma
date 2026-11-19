//! Explicit availability comparison; uses the real public middleware and Mongo.
use super::*;
use axum::http::StatusCode;
use std::{collections::BTreeMap, time::Instant};

#[test]
#[ignore = "requires a disposable constrained Mongo replica set"]
fn personal_inventory_constrained_availability_comparison() {
    let workers = std::env::var("CMA_LIST_TEST_WORKERS")
        .ok()
        .map(|value| value.parse::<usize>().unwrap())
        .unwrap_or(4);
    assert!((1..=4).contains(&workers));
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(workers)
        .enable_all()
        .build()
        .unwrap()
        .block_on(compare());
}

async fn compare() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .try_init();
    let f = fixture::Fixture::readers(100, 201, true).await;
    let mode = std::env::var("CMA_LIST_TEST_MODE").unwrap_or_else(|_| "both".into());
    for provenance in [false, true] {
        if std::env::var("CMA_LIST_TEST_ROUTE").is_ok_and(|v| v == "provenance") && !provenance {
            continue;
        }
        let path = if provenance {
            "/api/v1/agents/provenance?limit=200"
        } else {
            "/api/v1/agents?limit=200"
        };
        for batched in [false, true] {
            if (mode == "legacy" && batched) || (mode == "batch" && !batched) {
                continue;
            }
            let app = f.app(batched, false);
            // Warm each reader's admission bucket and retain its exact public reference.
            let mut expected_pages = Vec::new();
            for reader in 0..100 {
                let cookie = identity::cookie(reader);
                let expected = tokio::time::timeout(
                    std::time::Duration::from_secs(60),
                    public::request_cookie(&app, path, &cookie),
                )
                .await
                .expect("warm request must settle within diagnostic deadline");
                if reader % 20 == 0 {
                    f.meter.take();
                    eprintln!("AVAILABILITY_WARM batched={batched} path={path} reader={reader}");
                }
                assert_eq!(
                    expected.0,
                    StatusCode::OK,
                    "warm reader {reader}: {expected:?}"
                );
                assert_eq!(expected.1["items"].as_array().unwrap().len(), 200);
                expected_pages.push((cookie, expected));
            }
            f.meter.take();
            eprintln!("AVAILABILITY_START batched={batched} path={path}");
            let barrier = Arc::new(tokio::sync::Barrier::new(101));
            let mut readers = tokio::task::JoinSet::new();
            for (cookie, expected) in expected_pages {
                let (app, barrier) = (app.clone(), barrier.clone());
                readers.spawn(async move {
                    barrier.wait().await;
                    let start = Instant::now();
                    let result = tokio::time::timeout(
                        std::time::Duration::from_secs(60),
                        public::request_cookie(&app, path, &cookie),
                    )
                    .await
                    .unwrap_or_else(|_| {
                        (
                            StatusCode::GATEWAY_TIMEOUT,
                            serde_json::json!({"code":"diagnostic_60s_deadline"}),
                        )
                    });
                    if result.0 == StatusCode::OK {
                        assert_eq!(result, expected);
                    }
                    (
                        result.0.as_u16(),
                        result.1.get("code").cloned(),
                        start.elapsed().as_micros(),
                    )
                });
            }
            barrier.wait().await;
            let mut statuses = BTreeMap::<u16, usize>::new();
            let mut times = Vec::new();
            while let Some(row) = readers.join_next().await {
                let (status, code, time) = row.unwrap();
                *statuses.entry(status).or_default() += 1;
                if status != 200 {
                    println!("AVAILABILITY_FAILURE batched={batched} path={path} status={status} code={code:?} elapsed_us={time}");
                }
                times.push(time);
            }
            times.sort_unstable();
            let stats = f.meter.take();
            println!("AVAILABILITY batched={batched} path={path} statuses={statuses:?} p95_us={} p99_us={} stats={stats:?}", times[94], times[98]);
        }
    }
    if tokio::time::timeout(std::time::Duration::from_secs(10), f.close())
        .await
        .is_err()
    {
        eprintln!(
            "AVAILABILITY_CLEANUP diagnostic client did not settle; runner owns fixture cleanup"
        );
    }
}
