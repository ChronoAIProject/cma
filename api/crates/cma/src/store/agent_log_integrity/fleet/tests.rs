use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn stages_are_stable_nested_and_cover_more_than_the_explicit_limit() {
    let stages = [0, 10, 50, 100].map(Activation::new);
    let mut counts = [0; 4];
    for index in 0..1_000 {
        let id = AgentId(format!("agt_fleet_{index}"));
        let selected = stages.each_ref().map(|stage| stage.selected(&id));
        assert!(!selected[0]);
        assert!(selected[3]);
        assert!(!selected[1] || selected[2]);
        for (i, selected) in selected.into_iter().enumerate() {
            assert_eq!(selected, Activation::new(stages[i].percent).selected(&id));
            counts[i] += usize::from(selected);
        }
    }
    assert!((60..=140).contains(&counts[1]));
    assert!((400..=600).contains(&counts[2]));
    assert_eq!(counts[3], 1_000);
}

#[tokio::test]
async fn disabled_and_busy_preparation_never_poll_work() {
    let id = AgentId("agt_prepare".into());
    Activation::default()
        .prepare(&id, async { panic!("default-off preparation") })
        .await;
    let activation = Activation::new(100);
    let permit = activation.slots.acquire().await.unwrap();
    for index in 0..171 {
        activation
            .prepare(&AgentId(format!("agt_{index}")), async {
                panic!("busy preparation")
            })
            .await;
    }
    assert!(activation.cooldowns.lock().unwrap().is_empty());
    drop(permit);
    activation
        .prepare(&id, async { Ok(Prepared::Audited) })
        .await;
    activation
        .prepare(&id, async { panic!("duplicate prepared Agent") })
        .await;
}

#[tokio::test]
async fn failure_is_per_agent_and_cooldowns_are_bounded() {
    let activation = Activation::new(100);
    let failed = AgentId("agt_failed".into());
    activation.prepare(&failed, async { Err(()) }).await;
    activation
        .prepare(&failed, async { panic!("failed Agent cooldown") })
        .await;
    activation
        .prepare(&AgentId("agt_healthy".into()), async {
            Ok(Prepared::Reused)
        })
        .await;
    assert_eq!(activation.slots.available_permits(), 1);
    activation
        .cooldowns
        .lock()
        .unwrap()
        .insert(failed.clone(), Instant::now());
    activation
        .prepare(&failed, async { Ok(Prepared::Audited) })
        .await;
    for index in 0..MAX_CACHED_AGENTS + 20 {
        activation.remember(
            &AgentId(format!("agt_cache_{index}")),
            Instant::now() + SUCCESS_COOLDOWN,
        );
    }
    assert_eq!(
        activation.cooldowns.lock().unwrap().len(),
        MAX_CACHED_AGENTS
    );
}

#[tokio::test(start_paused = true)]
async fn timed_out_preparation_releases_the_global_slot() {
    let activation = Activation::new(100);
    activation
        .prepare(&AgentId("agt_slow".into()), std::future::pending())
        .await;
    assert_eq!(activation.slots.available_permits(), 1);
    activation
        .prepare(&AgentId("agt_other".into()), async {
            Ok(Prepared::Audited)
        })
        .await;
}

#[tokio::test]
async fn cancellation_and_concurrent_requests_keep_one_preparation() {
    let activation = Arc::new(Activation::new(100));
    let started = Arc::new(tokio::sync::Notify::new());
    let work = Arc::new(AtomicUsize::new(0));
    let task = {
        let activation = activation.clone();
        let started = started.clone();
        let work = work.clone();
        tokio::spawn(async move {
            activation
                .prepare(&AgentId("agt_active".into()), async {
                    work.fetch_add(1, Ordering::SeqCst);
                    started.notify_one();
                    std::future::pending().await
                })
                .await;
        })
    };
    started.notified().await;
    let mut peers = Vec::new();
    for index in 0..20 {
        let activation = activation.clone();
        peers.push(tokio::spawn(async move {
            let id = if index % 2 == 0 {
                "agt_active".into()
            } else {
                format!("agt_peer_{index}")
            };
            activation
                .prepare(&AgentId(id), async { panic!("concurrent preparation") })
                .await;
        }));
    }
    for peer in peers {
        peer.await.unwrap();
    }
    assert_eq!(work.load(Ordering::SeqCst), 1);
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(activation.slots.available_permits(), 1);
    activation
        .prepare(&AgentId("agt_healthy".into()), async {
            Ok(Prepared::Audited)
        })
        .await;
    activation
        .prepare(&AgentId("agt_active".into()), async {
            panic!("cancelled Agent cooldown")
        })
        .await;
}
