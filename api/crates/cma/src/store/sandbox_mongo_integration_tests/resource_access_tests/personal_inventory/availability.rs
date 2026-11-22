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
    let setting = |name| {
        std::env::var_os(name)
            .map(|value| {
                value
                    .into_string()
                    .expect("availability setting must be UTF-8")
            })
            .unwrap_or_else(|| "both".into())
    };
    let selected = qualification::availability_selection(
        &setting("CMA_LIST_TEST_MODE"),
        &setting("CMA_LIST_TEST_ROUTE"),
    )
    .expect("availability mode/route must be explicit supported values");
    let f = fixture::Fixture::readers(100, 201, true).await;
    let mut records = Vec::new();
    for (provenance, batched) in selected {
        let route = if provenance { "provenance" } else { "agents" };
        let mode = if batched { "batch" } else { "legacy" };
        let mut record = qualification::Record::new(
            "personal_inventory::availability::personal_inventory_constrained_availability_comparison",
            &format!("{mode}/{route}"), serde_json::json!({"kind":"availability", "route":route, "mode":mode,
                "distinct":true, "instrumentation":"command-observer", "expected_readers":100,
                "expected_warmup":100, "deadline_us":60_000_000}));
        let path = if provenance {
            "/api/v1/agents/provenance?limit=200"
        } else {
            "/api/v1/agents?limit=200"
        };
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
            record.set("warmup_completed", reader + 1);
        }
        f.meter.take();
        eprintln!("AVAILABILITY_START batched={batched} path={path}");
        let barrier = Arc::new(tokio::sync::Barrier::new(101));
        let mut readers = tokio::task::JoinSet::new();
        for (reader, (cookie, expected)) in expected_pages.into_iter().enumerate() {
            let (app, barrier) = (app.clone(), barrier.clone());
            readers.spawn(async move {
                barrier.wait().await;
                let start = Instant::now();
                let result = tokio::time::timeout(
                    std::time::Duration::from_secs(60),
                    public::request_cookie(&app, path, &cookie),
                )
                .await;
                let deadline = result.is_err();
                let result = result.unwrap_or_else(|_| {
                    (
                        StatusCode::GATEWAY_TIMEOUT,
                        serde_json::json!({"code":"diagnostic_60s_deadline"}),
                    )
                });
                let semantic_match = result == expected;
                qualification::Sample {
                    reader,
                    status: result.0.as_u16(),
                    elapsed_us: start.elapsed().as_micros(),
                    deadline,
                    semantic_match,
                }
            });
        }
        barrier.wait().await;
        let mut statuses = BTreeMap::<u16, usize>::new();
        let mut times = Vec::new();
        while let Some(row) = readers.join_next().await {
            let sample = row.unwrap();
            let valid = sample.status != 200 || sample.semantic_match;
            *statuses.entry(sample.status).or_default() += 1;
            times.push(sample.elapsed_us);
            record.sample(sample);
            assert!(
                valid,
                "successful availability page differs from its reference"
            );
        }
        times.sort_unstable();
        let stats = f.meter.take();
        println!("AVAILABILITY batched={batched} path={path} statuses={statuses:?} p95_us={} p99_us={} stats={stats:?}", times[94], times[98]);
        record.set("commands", &stats.commands);
        record.set("failed_commands", stats.failed);
        record.set("status_counts", &statuses);
        record.set("diagnostic_complete", true);
        record.set(
            "qualification_verdict",
            "requires-build-and-fixture-validation",
        );
        records.push(record);
    }
    let cleanup = tokio::time::timeout(std::time::Duration::from_secs(10), f.close())
        .await
        .is_ok();
    for record in &mut records {
        record.complete(cleanup);
    }
    if !cleanup {
        eprintln!(
            "AVAILABILITY_CLEANUP diagnostic client did not settle; runner owns fixture cleanup"
        );
    }
    println!("AVAILABILITY_DIAGNOSTIC_COMPLETE qualification=not-evaluated; use mongo-contract-tests.sh --qualify --availability");
}
