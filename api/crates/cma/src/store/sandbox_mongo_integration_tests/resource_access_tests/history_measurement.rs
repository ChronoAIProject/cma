use super::*;
use layer2_core::sandbox::{agent_io::*, *};
use std::time::Instant;

#[path = "history_fixture.rs"]
mod fixture;
#[path = "history_meter.rs"]
mod meter;
use fixture::Fixture;

const SAMPLES: usize = 20;

async fn measure(rows: u64) {
    let f = Fixture::new(rows).await;
    // Measure the authorization-session switch independently of page composition.
    let apps = [f.app(false), f.app(true)];
    for route in ["snapshot", "head_active", "head_idle"] {
        if route == "head_idle" {
            f.idle().await;
        }
        let endpoint = if route == "snapshot" {
            "snapshot"
        } else {
            "head"
        };
        let path = format!("/api/v2/agents/{}/events/{endpoint}", f.agent.id);
        let mut pages = Vec::new();
        let mut next = Some(path.clone());
        while let Some(path) = next {
            let expected = f.request(&apps[0], &path).await;
            assert_eq!(expected, f.request(&apps[1], &path).await);
            let body: serde_json::Value = serde_json::from_slice(&expected).unwrap();
            if pages.is_empty() {
                if route == "snapshot" {
                    let items = body["items"].as_array().unwrap();
                    assert!(items
                        .iter()
                        .any(|item| item["response_binding"].is_object()));
                    assert!(items
                        .iter()
                        .any(|item| item["message_command_id"].is_string()));
                } else if route == "head_active" {
                    assert!(body["active_turn"]["response_binding"].is_object());
                } else {
                    assert!(body["active_turn"].is_null());
                }
            }
            next = body["next_page_cursor"].as_str().map(|cursor| {
                format!(
                    "/api/v2/agents/{}/events/snapshot?page_cursor={cursor}",
                    f.agent.id
                )
            });
            pages.push((path, expected));
        }
        assert_eq!(
            pages.len(),
            if route == "snapshot" {
                rows.div_ceil(100) as usize
            } else {
                1
            }
        );
        let mut samples: [Vec<u128>; 2] = Default::default();
        let mut counts = std::collections::BTreeMap::new();
        for sample in 0..SAMPLES {
            for offset in 0..2 {
                let mode = (sample + offset) % 2;
                for (page, (path, expected)) in pages.iter().enumerate() {
                    f.meter.clear();
                    let scan_before = scans();
                    let start = Instant::now();
                    assert_eq!(f.request(&apps[mode], &path).await, *expected);
                    samples[mode].push(start.elapsed().as_micros());
                    let stats = f.meter.stats.lock().unwrap().clone();
                    let scan_after = scans();
                    let scan_delta: Vec<_> = scan_after
                        .iter()
                        .zip(scan_before)
                        .map(|(a, b)| a - b)
                        .collect();
                    let snapshots = match route {
                        "snapshot" => 8,
                        "head_active" => 7,
                        _ => 4,
                    };
                    assert_eq!(stats.snapshots, snapshots);
                    assert_eq!(stats.commands.values().sum::<u64>(), stats.checkouts);
                    let count = (
                        stats.commands.clone(),
                        stats.docs,
                        stats.bytes,
                        scan_delta[..3].to_vec(),
                    );
                    if let Some(expected) = counts.get(&(mode, page)) {
                        assert_eq!(&count, expected);
                    } else {
                        counts.insert((mode, page), count);
                    }
                    if sample == 0 {
                        println!("HISTORY_COMMANDS rows={rows} route={route} page={page} access_session={} page_single_fence=false snapshots={} commands={} auth_commands={} returned_docs={} returned_bson_bytes={} full_scan_counts_docs_bytes_us={:?} groups={:?}",
                            mode == 1, stats.snapshots, stats.checkouts, stats.auth_commands,
                            stats.docs, stats.bytes, scan_delta, stats.commands);
                    }
                }
            }
        }
        for (mode, values) in samples.iter_mut().enumerate() {
            values.sort_unstable();
            println!("HISTORY_LATENCY rows={rows} route={route} access_session={} page_single_fence=false samples={} p50_us={} p95_us={} max_us={}",
                mode == 1, values.len(), values[values.len().div_ceil(2)-1], values[(values.len()*95).div_ceil(100)-1], values[values.len()-1]);
        }
    }
    f.close().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn history_page_measurement_small() {
    crate::store::agent_log_telemetry::isolated_test(measure(260)).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn history_page_measurement_large() {
    crate::store::agent_log_telemetry::isolated_test(measure(1000)).await;
}

fn scans() -> [u64; 4] {
    crate::store::agent_log_telemetry::snapshot(
        crate::store::agent_log_telemetry::ScanCaller::PublicWindowPrefix,
        true,
    )
}
