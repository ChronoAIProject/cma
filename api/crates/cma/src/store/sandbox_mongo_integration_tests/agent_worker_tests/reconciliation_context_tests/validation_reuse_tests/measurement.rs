use super::*;
use std::time::Instant;

fn cpu() -> f64 {
    let output = std::process::Command::new("ps")
        .args(["-o", "time=", "-p", &std::process::id().to_string()])
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .unwrap()
        .trim()
        .split(':')
        .fold(0., |n, part| n * 60. + part.parse::<f64>().unwrap())
}
async fn scan(store: &MongoSandboxStore) -> Vec<AgentConnectionRecoveryCandidate> {
    let mut after = None;
    let mut result = Vec::new();
    loop {
        let page = store
            .live_agent_connections(&PageRequest::new(100, after).unwrap())
            .await
            .unwrap();
        result.extend(page.items);
        match page.next {
            Some(next) => after = Some(next),
            None => return result,
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn two_hundred_candidates_preserve_rows_and_measure_cpu_with_foreground_reads() {
    let (client, name) = database("validation_measurement").await;
    let baseline = MongoSandboxStore::open(&client, &name).await.unwrap();
    let seed = idle_connection_fixture(&baseline, "measurement", AgentStatus::Ready).await;
    let mut rows = Vec::new();
    for n in 1..200 {
        let mut agent = seed.clone();
        agent.id = AgentId(format!("agt_measurement_{n:04}"));
        agent.worktree_id = WorktreeId(format!("wkt_measurement_{n:04}"));
        agent.creation_key = format!("measurement-{n}");
        agent.conversation.conversation_id = Some(format!("measurement-{n}"));
        rows.push(crate::store::agents::agent_document(&agent).unwrap());
    }
    baseline.agents.insert_many(rows).await.unwrap();
    let expected = scan(&baseline).await;
    assert_eq!(expected.len(), 200);
    let cached = baseline.clone().with_agent_connection_observations(true);
    let other_replica = baseline.clone().with_agent_connection_observations(true);
    assert_eq!(scan(&cached).await, expected);
    assert_eq!(scan(&other_replica).await, expected);
    // Simultaneous bounded scanner concurrency over 200 distinct conversations;
    // independent process caches must still return the same complete population.
    let (a, b, c, d) = tokio::join!(
        scan(&cached),
        scan(&cached),
        scan(&other_replica),
        scan(&other_replica)
    );
    for rows in [a, b, c, d] {
        assert_eq!(rows, expected);
    }
    for (phase, store) in [
        ("off_a", &baseline),
        ("on_b", &cached),
        ("on_b2", &cached),
        ("off_a2", &baseline),
    ] {
        let cpu_before = cpu();
        let started = Instant::now();
        let mut request_ms = Vec::new();
        let mut scan_ms = Vec::new();
        let before = store
            .agent_connection_observations
            .as_ref()
            .map(|_| counts(store));
        crate::store::mongo_read_telemetry::isolated_test(async {
            for _ in 0..40 {
                let (rows, (foreground, elapsed)) = tokio::join!(async {
                    let at = Instant::now();
                    let rows = scan(store).await;
                    scan_ms.push(at.elapsed().as_secs_f64() * 1000.);
                    rows
                }, async {
                    let at = Instant::now();
                    let value = baseline.get_agent(&seed.id).await.unwrap();
                    (value, at.elapsed().as_secs_f64() * 1000.)
                });
                assert_eq!(rows, expected);
                assert_eq!(foreground.as_ref(), Some(&seed));
                request_ms.push(elapsed);
            }
            let observed = crate::store::mongo_read_telemetry::snapshot(AGENTS);
            // Two pages, including the first page's hidden lookahead, per sweep.
            assert_eq!(observed[2], 40 * 201);
            request_ms.sort_by(f64::total_cmp);
            scan_ms.sort_by(f64::total_cmp);
            println!("VALIDATION_REUSE_MEASUREMENT {}", serde_json::json!({
                "phase":phase,"candidates":200,"sweeps":40,"documents":observed[2],"bytes":observed[3],
                "wall_ms":started.elapsed().as_secs_f64()*1000.,"process_cpu_ms":(cpu()-cpu_before)*1000.,
                "scan_p95_ms":scan_ms[37],"foreground_p95_ms":request_ms[37],
                "retained":store.agent_connection_observations.as_ref().map(|c| c.retained()),"cache_before":before,"cache_after":store.agent_connection_observations.as_ref().map(|_| counts(store)),
            }));
        }).await;
    }
    assert!(counts(&cached).1 >= 8000);
    client.database(&name).drop().await.unwrap();
}
