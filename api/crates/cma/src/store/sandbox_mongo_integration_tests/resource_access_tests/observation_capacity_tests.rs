use super::*;
use layer2_core::sandbox::agent_observation::observation_scope;

pub(super) async fn available(replicas: &[Server], expected: &[usize]) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if replicas
                .iter()
                .map(|replica| replica.relay.available_test_slots())
                .eq(expected.iter().copied())
            {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable Mongo replica-set runner"]
async fn observation_relay_admits_once_retains_bodies_and_preserves_control_reserve() {
    let mut f = Fixture::semantic(100).await;
    agents(&mut f, false, false).await;
    let replicas = servers_with_capacity(
        &f,
        true,
        crate::config::RelayCapacityConfig {
            limit: 2,
            control_reserve: 1,
        },
    )
    .await;
    let destination = replicas[0]
        .observation
        .destination(&f.agent.id, &f.agent.created_at)
        .await
        .unwrap();
    let owner = if destination.authority.token.instance_id == "observation-0" {
        0
    } else {
        1
    };
    let peer = 1 - owner;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let path = format!(
        "/api/v1/agui/events?thread_id={}&starting_after=100&follow=true",
        f.agent.id
    );
    let request = |index: usize| {
        client
            .get(format!("{}{}", replicas[index].address, path))
            .bearer_auth(format!("observer:{}", f.agent.owner))
    };
    let local = request(owner).send().await.unwrap();
    assert_eq!(local.status(), reqwest::StatusCode::OK);
    let mut expected = [2, 2];
    expected[owner] = 1;
    available(&replicas, &expected).await;

    // The local hint subscription is immediately ready and consumes no second
    // slot: the enclosing response already owns the observer admission.
    {
        let admitted = crate::agent_manager::relay::admitted_observation(
            f.agent.id.clone(),
            observation_scope(
                destination.authority,
                replicas[owner].updates.admit_live(&f.agent.id),
            ),
        );
        tokio::pin!(admitted);
        let std::task::Poll::Ready(Ok(Some(live))) = futures_util::poll!(&mut admitted) else {
            panic!("read-owner local admission must not wait or acquire a second slot");
        };
        drop(live);
    }
    available(&replicas, &expected).await;
    for index in [owner, peer] {
        let refused = request(index).send().await.unwrap();
        assert_eq!(refused.status(), reqwest::StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(refused.headers()["retry-after"], "1");
        let body: serde_json::Value = refused.json().await.unwrap();
        assert_eq!(body["code"], "relay_observer_capacity_exhausted");
    }
    available(&replicas, &expected).await;
    // Invalid control input must reach the unchanged validator even when all
    // observer capacity is occupied; observer pressure cannot refuse approval.
    for index in [owner, peer] {
        let control = client
        .post(format!("{}/api/v1/agui/run", replicas[index].address))
        .bearer_auth(format!("observer:{}", f.agent.owner))
        .json(&serde_json::json!({"threadId":f.agent.id,"resume":[{"interruptId":"one"},{"interruptId":"two"}]}))
        .send().await.unwrap();
        assert_eq!(control.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);
        assert!(control
            .text()
            .await
            .unwrap()
            .contains("invalid_run_request"));
    }
    drop(local);
    available(&replicas, &[2, 2]).await;
    let remote = request(peer).send().await.unwrap();
    assert_eq!(remote.status(), reqwest::StatusCode::OK);
    available(&replicas, &[1, 1]).await;
    drop(remote);
    available(&replicas, &[2, 2]).await;
    assert_eq!(
        f.client
            .database(&f.name)
            .collection::<Document>("cma_agent_manager_owners")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0,
        "read custody never acquires runtime ownership or continuity capacity"
    );
    drop(replicas);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable Mongo replica-set runner"]
async fn observation_relay_unanswered_owner_has_three_second_admission_bound() {
    let mut f = Fixture::semantic(100).await;
    agents(&mut f, false, false).await;
    let replicas = servers(&f).await;
    let destination = replicas[0]
        .observation
        .destination(&f.agent.id, &f.agent.created_at)
        .await
        .unwrap();
    let owner = if destination.authority.token.instance_id == "observation-0" {
        0
    } else {
        1
    };
    replicas[owner]
        .stall
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let started = Instant::now();
    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/agui/events?thread_id={}&starting_after=100&follow=true",
            replicas[1 - owner].address,
            f.agent.id
        ))
        .bearer_auth(format!("observer:{}", f.agent.owner))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    assert!(started.elapsed() >= Duration::from_secs(3));
    assert!(started.elapsed() < Duration::from_secs(5));
    let body = response.text().await.unwrap();
    assert!(body.contains("agent_owner_unavailable"));
    assert!(!body.contains("RUN_ERROR"));
    available(&replicas, &[256, 256]).await;
    drop(replicas);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable Mongo replica-set runner with failpoints"]
async fn observation_forwarded_cold_reconstruction_outlives_admission_deadline() {
    for shared in [false, true] {
        let mut f = Fixture::semantic(10_000).await;
        agents(&mut f, false, false).await;
        let subject = if shared {
            let snapshot = f
                .store
                .resource_access_snapshot(&ResourceTarget::Agent {
                    agent_id: f.agent.id.clone(),
                })
                .await
                .unwrap();
            f.store
                .mutate_share(super::super::super::mutation(
                    &snapshot,
                    "cold-viewer",
                    ShareMode::View,
                ))
                .await
                .unwrap();
            SubjectId::new("shared-reader")
        } else {
            f.agent.owner.clone()
        };
        let replicas = budget_servers(&f).await;
        let destination = replicas[0]
            .observation
            .destination(&f.agent.id, &f.agent.created_at)
            .await
            .unwrap();
        let owner = usize::from(destination.authority.token.instance_id == "observation-1");
        // Delay only retained history reads. Admission, grant revalidation and
        // membership refresh are separate from the cold replay under test.
        let admin = f.client.database("admin");
        let delayed_reads = 100_i32;
        let configured = admin
            .run_command(doc! {
                "configureFailPoint": "failCommand", "mode": {"times": delayed_reads},
                "data": {"failCommands": ["find"], "namespace": format!("{}.agent_log", f.name),
                         "blockConnection": true, "blockTimeMS": 80}
            })
            .await
            .unwrap();
        f.meter.clear();
        let started = Instant::now();
        let opened = tokio::time::timeout(
            Duration::from_secs(30),
            reqwest::Client::new()
                .get(format!(
                    "{}/api/v1/agui/events?thread_id={}&starting_after=10000&follow=true",
                    replicas[1 - owner].address,
                    f.agent.id
                ))
                .bearer_auth(format!("observer:{}", subject))
                .send(),
        )
        .await;
        let elapsed = started.elapsed();
        let cleared = admin
            .run_command(doc! {
                "configureFailPoint": "failCommand", "mode": "off"
            })
            .await
            .unwrap();
        let count = |reply: &Document| {
            reply
                .get_i64("count")
                .or_else(|_| reply.get_i32("count").map(i64::from))
                .unwrap()
        };
        let blocked = count(&cleared) - count(&configured);
        let response = opened
            .expect("cold replay exceeded its existing transport bound")
            .unwrap();
        let status = response.status();
        if status != reqwest::StatusCode::OK {
            panic!(
                "forwarded shared={shared} cold replay returned {status} after {elapsed:?} with {blocked} delayed reads: {}",
                response.text().await.unwrap()
            );
        }
        assert_eq!(blocked, i64::from(delayed_reads));
        assert_eq!(response.headers()["content-type"], "text/event-stream");
        assert!(!response.headers().contains_key("x-cma-read-admission"));
        let log_reads = f.meter.stats.lock().unwrap().commands["log/agent_log/find"];
        assert!(
            log_reads >= 50,
            "cold reconstruction must actually read the 10k prefix"
        );
        assert!(elapsed > Duration::from_secs(3));
        available(&replicas, &[99, 99]).await;
        eprintln!("FORWARDED_COLD_REPLAY shared={shared} rows=10000 status={status} elapsed={elapsed:?} log_reads={log_reads} delayed_reads={blocked} available_slots=[99,99] admission_deadline=3s");
        drop(response);
        available(&replicas, &[100, 100]).await;
        drop(replicas);
        f.close().await;
    }
}
