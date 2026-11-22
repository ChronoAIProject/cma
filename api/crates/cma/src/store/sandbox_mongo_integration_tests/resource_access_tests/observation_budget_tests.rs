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
    let mut record = qualification::Record::new(
        if seconds == 1800 {
            "history_measurement::observation_budget::observation_one_hundred_observers_on_two_replicas_for_thirty_minutes"
        } else {
            "history_measurement::observation_budget::observation_two_serving_replicas_keep_idle_work_independent_of_history"
        },
        if distinct {
            "distinct-owners"
        } else if many {
            "many-agents"
        } else {
            "one-agent"
        },
        serde_json::json!({"kind":"observation", "history":rows, "many":many, "distinct":distinct,
            "instrumentation":"command-observer", "expected_observers":100, "replicas":2,
            "required_seconds":seconds, "opening_concurrency":16}),
    );
    let mut f = Fixture::semantic(rows).await;
    let agents = agents(&mut f, many, distinct).await;
    let replicas = budget_servers(&f).await;
    let client = reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    let mut consumers = Vec::new();
    // Opening is measured separately, never confused with steady idle work.
    f.meter.clear();
    let opening = (0..100).map(|index| {
        let (agents, replicas, client) = (&agents, &replicas, &client);
        async move {
            let agent = &agents[index % agents.len()];
            let path = if index % 2 == 0 {
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
            let url = format!("{}{}", replicas[index % 2].address, path);
            let started = Instant::now();
            let response = tokio::time::timeout(
                Duration::from_secs(15),
                client
                    .get(url)
                    .bearer_auth(format!("observer:{}", agent.owner))
                    .send(),
            )
            .await
            .unwrap()
            .unwrap();
            let status = response.status();
            if status != reqwest::StatusCode::OK {
                let slots: Vec<_> = replicas.iter().map(|server| server.relay.available_test_slots()).collect();
                let body = response.text().await.unwrap();
                panic!("observer {index}: status={status} history={rows} elapsed={:?} available_slots={slots:?} body={body}", started.elapsed());
            }
            (index, response, started.elapsed())
        }
    });
    let mut longest_open = Duration::ZERO;
    for (reader, response, elapsed) in futures_util::stream::iter(opening)
        .buffer_unordered(16)
        .collect::<Vec<_>>()
        .await
    {
        longest_open = longest_open.max(elapsed);
        record.sample(qualification::Sample {
            reader,
            status: 200,
            elapsed_us: elapsed.as_micros(),
            deadline: false,
            semantic_match: true,
        });
        consumers.push(tokio::spawn(async move {
            let mut body = response.bytes_stream();
            while let Some(chunk) = body.next().await {
                chunk.unwrap();
            }
            panic!("idle shared stream closed unexpectedly");
        }));
    }
    let cold = f.meter.stats.lock().unwrap().clone();
    record.set("opening_commands", &cold.commands);
    record.set("agents", agents.len());
    eprintln!(
        "shared_open agents={} observers=100 history={rows} distinct={distinct} commands={} longest_open={longest_open:?}",
        agents.len(),
        cold.commands.values().sum::<u64>()
    );
    // A remote observer holds one slot on ingress and one on its read owner;
    // a local observer holds one total, including its local hint subscription.
    let mut owners = Vec::new();
    for agent in &agents {
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
    tokio::time::sleep(Duration::from_secs(2)).await;
    f.meter.clear();
    let started = Instant::now();
    record.set("stable_started_ms", qualification::now_ms());
    for _ in 0..seconds {
        tokio::time::sleep(Duration::from_secs(1)).await;
        record.set("stable_elapsed_us", started.elapsed().as_micros());
        record.set(
            "active_observers",
            consumers.iter().filter(|task| !task.is_finished()).count(),
        );
        assert!(
            consumers.iter().all(|task| !task.is_finished()),
            "all 100 observers must remain connected"
        );
    }
    let elapsed = started.elapsed().as_secs_f64();
    let stats = f.meter.stats.lock().unwrap().clone();
    let commands = stats.commands.values().sum::<u64>();
    record.set("stable_finished_ms", qualification::now_ms());
    record.set("stable_elapsed_us", started.elapsed().as_micros());
    record.set("commands", &stats.commands);
    record.set("snapshots", stats.snapshots);
    record.set("bytes", stats.bytes);
    record.set("head_agents", stats.per_agent_heads.len());
    record.set(
        "max_head_refreshes",
        stats
            .per_agent_heads
            .values()
            .map(Vec::len)
            .max()
            .unwrap_or(0),
    );
    record.set(
        "minimum_refresh_gap_us",
        stats
            .per_agent_heads
            .values()
            .flat_map(|times| {
                times
                    .windows(2)
                    .map(|pair| pair[1].duration_since(pair[0]).as_micros())
            })
            .min(),
    );
    assert!(
        (commands as f64 / elapsed) <= 100.0,
        "{commands} commands/{elapsed}s: {:?}",
        stats.commands
    );
    assert_eq!(stats.per_agent_heads.len(), agents.len());
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
    assert!(stats.snapshots <= agents.len() as u64 * (seconds + 1));
    assert!(
        stats
            .commands
            .keys()
            .all(|name| !name.contains("agent_log")),
        "idle streams must not traverse history: {:?}",
        stats.commands
    );
    eprintln!("shared_idle agents={} observers=100 replicas=2 distinct={distinct} history={rows} seconds={elapsed:.3} commands={commands} commands_per_second={:.3} head_cohort_refreshes={} bytes={} details={:?}", agents.len(), commands as f64 / elapsed, stats.snapshots, stats.bytes, stats.commands);
    for consumer in consumers {
        consumer.abort();
        let _ = consumer.await;
    }
    drop(replicas);
    tokio::time::sleep(Duration::from_millis(50)).await;
    f.client.database(&f.name).drop().await.unwrap();
    record.complete(true);
    stats
}

#[tokio::test]
#[ignore = "requires the disposable Mongo replica-set runner"]
async fn observation_two_serving_replicas_keep_idle_work_independent_of_history() {
    // Cover multiple three-second membership and ten-second driver heartbeat
    // periods so a boundary crossing cannot dominate the history-growth ratio.
    let h = measure(1_000, false, false, 30).await;
    let ten_h = measure(10_000, false, false, 30).await;
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
