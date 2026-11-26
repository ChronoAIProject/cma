//! Complete public streams on two listening replicas; the driver meter includes
//! membership, read custody, routing, release fences and all getMore commands.
use super::*;
use crate::{
    agent_manager::relay::Relay,
    store::{agent_observation::MongoAgentObservation, agent_ownership::MongoAgentOwnership},
};
use futures_util::StreamExt;
use layer2_core::agent_ownership::{Member, OwnershipCoordinator, PROTOCOL_VERSION};
use layer2_core::sandbox::agent_observation::AgentObservationService;
use std::time::Duration;

const OBSERVATION_ROUTES: [&str; 2] = ["v1_agui", "v2_agent_events"];

#[path = "observation_fixture.rs"]
mod serving;
use serving::*;
#[path = "observation_capacity_tests.rs"]
mod observation_capacity;
#[path = "observation_refresh_tests.rs"]
mod observation_refresh;
#[path = "response_shared_tests.rs"]
mod response_shared;

async fn measure(rows: u64, many: bool, distinct: bool, seconds: u64) -> meter::Stats {
    measure_idle(rows, many, distinct, seconds, true).await.0
}

async fn measure_idle(
    rows: u64,
    many: bool,
    distinct: bool,
    seconds: u64,
    shared_observation: bool,
) -> (meter::Stats, Duration) {
    let mut f = Fixture::semantic(rows).await;
    let agents = agents(&mut f, many, distinct).await;
    let replicas = budget_servers_with_observation(&f, shared_observation).await;
    let client = reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    let mut consumers = Vec::new();
    // Opening is measured separately, never confused with steady idle work.
    // The unshared baseline seeks the retained prefix for every observer.
    let open_timeout = Duration::from_secs(if shared_observation { 15 } else { 30 });
    let concurrent_opens = match (shared_observation, rows) {
        (true, _) => 16,
        (false, 10_000) => 2,
        (false, _) => 4,
    };
    f.meter.start_opening();
    let cohort_started = Instant::now();
    let opening = (0..100).map(|index| {
        let (agents, replicas, client) = (&agents, &replicas, &client);
        async move {
            let agent = &agents[index % agents.len()];
            let replica = index % 2;
            let route = OBSERVATION_ROUTES[replica];
            let path = if replica == 0 {
                format!(
                    "/api/v1/agui/events?thread_id={}&starting_after={rows}&follow=true",
                    agent.id
                )
            } else {
                let bytes =
                    serde_json::to_vec(&("cma.agent.cursor.v1", &agent.id, &agent.created_at))
                        .unwrap();
                let identity = format!("{:x}", Sha256::digest(bytes));
                format!(
                    "/api/v2/agents/{}/events?after_cursor=ag1.{identity}.{rows}&follow=true",
                    agent.id
                )
            };
            let url = format!("{}{}", replicas[replica].address, path);
            let started = Instant::now();
            let open = tokio::time::timeout(
                open_timeout,
                client
                    .get(url)
                    .bearer_auth(format!("observer:{}", agent.owner))
                    .send(),
            )
            .await;
            let elapsed = started.elapsed();
            let failed = |reason: &str| {
                format!(
                    "observation_open_failed observer={index} route={route} replica={replica} \
                    history={rows} shared={shared_observation} distinct={distinct} \
                    elapsed={elapsed:?} cohort_elapsed={:?} reason={reason}",
                    cohort_started.elapsed(),
                )
            };
            let response = match open {
                Ok(Ok(response)) => response,
                Ok(Err(error)) if error.is_connect() => return Err(failed("connect")),
                Ok(Err(_)) => return Err(failed("transport")),
                Err(_) => return Err(failed("opening_deadline")),
            };
            let status = response.status();
            if status != reqwest::StatusCode::OK {
                return Err(failed(&format!("http_status_{status}")));
            }
            Ok((response, elapsed, replica))
        }
    });
    let mut opening = futures_util::stream::iter(opening).buffer_unordered(concurrent_opens);
    let mut opened = Vec::with_capacity(100);
    let mut count_by_route = [0; 2];
    let mut longest_by_route = [Duration::ZERO; 2];
    let report_opening = |phase, counts: [usize; 2], longest: [Duration; 2]| {
        let cohort_elapsed = cohort_started.elapsed();
        let mongo = f.meter.opening_stats();
        eprintln!(
            "observation_open phase={phase} shared={shared_observation} agents={} \
            observers=100 history={rows} distinct={distinct} commands={} \
            checkouts={} concurrent_opens={concurrent_opens} \
            longest_checkout={:?} longest_command={:?} \
            cohort_elapsed={cohort_elapsed:?} longest_open={:?}",
            agents.len(),
            mongo.commands,
            mongo.checkouts,
            mongo.longest_checkout,
            mongo.longest_command,
            longest[0].max(longest[1]),
        );
        for (replica, route) in OBSERVATION_ROUTES.into_iter().enumerate() {
            eprintln!(
                "observation_open_route phase={phase} route={route} replica={replica} history={rows} \
                shared={shared_observation} distinct={distinct} opened={} longest_open={:?}",
                counts[replica], longest[replica],
            );
        }
    };
    while let Some(result) = opening.next().await {
        let (response, elapsed, replica) = result.unwrap_or_else(|failure| {
            report_opening("failed", count_by_route, longest_by_route);
            panic!("{failure}");
        });
        count_by_route[replica] += 1;
        longest_by_route[replica] = longest_by_route[replica].max(elapsed);
        opened.push(response);
        if matches!(opened.len(), 25 | 50 | 75) && rows == 10_000 && !shared_observation {
            report_opening("progress", count_by_route, longest_by_route);
        }
    }
    drop(opening);
    report_opening("complete", count_by_route, longest_by_route);
    f.meter.finish_opening();
    for response in opened {
        consumers.push(tokio::spawn(async move {
            let mut body = response.bytes_stream();
            while let Some(chunk) = body.next().await {
                chunk.unwrap();
            }
            panic!("idle shared stream closed unexpectedly");
        }));
    }
    if shared_observation {
        assert_shared_slots(&replicas, &agents).await;
    }
    tokio::time::sleep(Duration::from_secs(2)).await;
    f.meter.clear();
    let started = Instant::now();
    for _ in 0..seconds {
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(
            consumers.iter().all(|task| !task.is_finished()),
            "all 100 observers must remain connected"
        );
    }
    let (elapsed, stats) = {
        let stats = f.meter.stats.lock().unwrap();
        (started.elapsed(), stats.clone())
    };
    for consumer in consumers {
        consumer.abort();
    }
    drop(replicas);
    tokio::time::sleep(Duration::from_millis(50)).await;
    f.client.database(&f.name).drop().await.unwrap();
    assert!(stats.checkouts > 0, "idle window recorded no checkouts");
    if shared_observation {
        assert_shared_idle(&stats, agents.len(), seconds, elapsed);
    }
    let commands = stats.commands.values().sum::<u64>();
    eprintln!("observation_idle shared={shared_observation} agents={} observers=100 replicas=2 distinct={distinct} history={rows} seconds={:.3} commands={commands} checkouts={} head_cohort_refreshes={} bytes={} details={:?}", agents.len(), elapsed.as_secs_f64(), stats.checkouts, stats.snapshots, stats.bytes, stats.commands);
    (stats, elapsed)
}

async fn assert_shared_slots(replicas: &[Server], agents: &[Agent]) {
    // A remote observer holds one slot on ingress and one on its read owner;
    // a local observer holds one total, including its local hint subscription.
    let mut owners = Vec::new();
    for agent in agents {
        let destination = replicas[0]
            .observation
            .destination(&agent.id, &agent.created_at)
            .await
            .unwrap();
        owners.push(
            replicas
                .iter()
                .position(|replica| replica.address == destination.member.relay_address)
                .unwrap(),
        );
    }
    let mut expected_slots = [100; 2];
    for index in 0..100 {
        let ingress = index % 2;
        let owner = owners[index % agents.len()];
        expected_slots[ingress] -= 1;
        if owner != ingress {
            expected_slots[owner] -= 1;
        }
    }
    assert_eq!(
        replicas
            .iter()
            .map(|replica| replica.relay.available_test_slots())
            .collect::<Vec<_>>(),
        expected_slots,
        "every observer consumes exactly one permit per participating replica"
    );
}

fn assert_shared_idle(stats: &meter::Stats, agent_count: usize, seconds: u64, elapsed: Duration) {
    let elapsed = elapsed.as_secs_f64();
    let commands = stats.commands.values().sum::<u64>();
    assert!(
        (commands as f64 / elapsed) <= 100.0,
        "{commands} commands/{elapsed}s: {:?}",
        stats.commands
    );
    assert_eq!(stats.per_agent_heads.len(), agent_count);
    for (id, times) in &stats.per_agent_heads {
        assert!(
            times.len() as u64 <= seconds + 1,
            "head refresh rate for {id}: {}",
            times.len()
        );
    }
    for times in stats.per_agent_heads.values() {
        assert!(
            times
                .windows(2)
                .all(|pair| pair[1].duration_since(pair[0]) >= Duration::from_secs(1)),
            "periodic refreshes must be separated by at least one second"
        );
    }
    assert!(stats.snapshots <= agent_count as u64 * (seconds + 1));
    assert!(
        stats
            .commands
            .keys()
            .all(|name| !name.contains("agent_log")),
        "idle streams must not traverse history: {:?}",
        stats.commands
    );
}

fn assert_checkout_reduction(
    rows: u64,
    baseline: &(meter::Stats, Duration),
    shared: &(meter::Stats, Duration),
) {
    let (baseline, baseline_elapsed) = baseline;
    let (shared, shared_elapsed) = shared;
    let baseline_rate = baseline.checkouts as f64 / baseline_elapsed.as_secs_f64();
    let shared_rate = shared.checkouts as f64 / shared_elapsed.as_secs_f64();
    assert!(
        shared_rate <= baseline_rate * 0.30,
        "history {rows}: expected at least 70% fewer idle checkouts/s, baseline {baseline_rate:.2}, shared {shared_rate:.2}"
    );
}

// Let the two in-process serving replicas make progress on separate workers.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the disposable Mongo replica-set runner"]
async fn observation_two_serving_replicas_keep_idle_work_independent_of_history() {
    // Cover multiple three-second membership and ten-second driver heartbeat
    // periods so a boundary crossing cannot dominate the history-growth ratio.
    let baseline = measure_idle(1_000, false, false, 30, false).await;
    let shared = measure_idle(1_000, false, false, 30, true).await;
    assert_checkout_reduction(1_000, &baseline, &shared);
    let (h, _) = shared;
    let shared = measure_idle(10_000, false, false, 30, true).await;
    let baseline = measure_idle(10_000, false, false, 30, false).await;
    assert_checkout_reduction(10_000, &baseline, &shared);
    let (ten_h, _) = shared;
    assert!(ten_h.commands.values().sum::<u64>() * 100 <= h.commands.values().sum::<u64>() * 110);
    measure(100, true, false, 10).await;
    measure(100, true, true, 10).await;
}

#[tokio::test]
#[ignore = "requires 30-minute disposable Mongo qualification"]
async fn observation_one_hundred_observers_on_two_replicas_for_thirty_minutes() {
    let (one, many, distinct) = tokio::join!(
        measure(1_000, false, false, 1800),
        measure(100, true, false, 1800),
        measure(100, true, true, 1800)
    );
    assert!(one.snapshots > 0 && many.snapshots > 0 && distinct.snapshots > 0);
}

#[tokio::test]
#[ignore = "requires the disposable Mongo replica-set runner"]
async fn observation_relay_refuses_cross_owner_reads_and_forged_proofs_before_payload_release() {
    let mut f = Fixture::semantic(100).await;
    let agents = agents(&mut f, true, true).await;
    let replicas = servers(&f).await;
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    for replica in &replicas {
        let path = format!(
            "{}/api/v1/agui/events?thread_id={}&starting_after=99&follow=false",
            replica.address, agents[1].id
        );
        let valid = client
            .get(&path)
            .bearer_auth(format!("observer:{}", agents[1].owner))
            .send()
            .await
            .unwrap();
        assert_eq!(valid.status(), reqwest::StatusCode::OK);
        assert!(valid.text().await.unwrap().contains("id: 100"));
        f.meter.clear();
        let denied = client
            .get(&path)
            .bearer_auth(format!("observer:{}", agents[0].owner))
            .send()
            .await
            .unwrap();
        assert_eq!(denied.status(), reqwest::StatusCode::NOT_FOUND);
        assert!(!denied.text().await.unwrap().contains("Fixture request"));
        assert!(f
            .meter
            .stats
            .lock()
            .unwrap()
            .commands
            .keys()
            .all(|key| !key.contains("agent_log")));
        let forged = client
            .get(&path)
            .bearer_auth(format!("observer:{}", agents[1].owner))
            .header("x-cma-agent-manager-relay", "forged-fixture-proof")
            .send()
            .await
            .unwrap();
        assert_eq!(forged.status(), reqwest::StatusCode::FORBIDDEN);
        let body = client
            .get(&path)
            .bearer_auth(format!("observer:{}", agents[1].owner))
            .body("unexpected")
            .send()
            .await
            .unwrap();
        assert_eq!(body.status(), reqwest::StatusCode::PAYLOAD_TOO_LARGE);
        // A routed POST preserves its body and reaches the unchanged public
        // input validator without admitting an effect from a read token.
        let post = client
            .post(format!("{}/api/v1/agui/run", replica.address))
            .bearer_auth(format!("observer:{}", agents[1].owner))
            .json(&serde_json::json!({"threadId": agents[1].id, "messages": []}))
            .send()
            .await
            .unwrap();
        assert_eq!(post.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);
        assert!(post.text().await.unwrap().contains("invalid_run_request"));
    }
    drop(replicas);
    f.client.database(&f.name).drop().await.unwrap();
}
