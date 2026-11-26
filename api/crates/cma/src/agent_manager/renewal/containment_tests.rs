use super::*;
use layer2_core::agent_ownership::{MaintenancePolicy, RetirementIntent};

#[tokio::test(start_paused = true)]
async fn periodic_cleanup_progresses_while_heartbeat_is_unavailable() {
    let store = DelayedStore::new(Duration::from_millis(25));
    store.unavailable_heartbeats.store(20, Ordering::SeqCst);
    store.heartbeat_delay_ms.store(1_900, Ordering::SeqCst);
    let coordinator = OwnershipCoordinator::register_with_policy(
        store,
        Member {
            instance_id: "manager".into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://manager.invalid".into(),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: DEFAULT_MAX_OWNED_AGENTS,
        },
        false,
        MaintenancePolicy::ContainFailures,
    )
    .await
    .unwrap();
    let mut pending = Vec::new();
    for index in 0..9 {
        let authority = coordinator
            .acquire(&AgentId(format!("pending-{index}")))
            .await
            .unwrap()
            .unwrap();
        authority
            .begin_retirement(RetirementIntent::Inactive)
            .unwrap();
        pending.push(authority);
    }
    let observations = Arc::new(Mutex::new(Vec::new()));
    let started = Instant::now();
    let running = tokio::spawn(run(coordinator.clone(), {
        let coordinator = coordinator.clone();
        let observations = observations.clone();
        move |_| {
            let coordinator = coordinator.clone();
            let observations = observations.clone();
            async move {
                let batch = coordinator.cancelled_receivers().await;
                observations.lock().unwrap().push((
                    Instant::now(),
                    batch
                        .into_iter()
                        .map(|a| a.token.agent_id.clone())
                        .collect::<Vec<_>>(),
                ));
                Ok(())
            }
        }
    }));
    tokio::time::sleep(Duration::from_millis(9_001)).await;
    let observations = observations.lock().unwrap().clone();
    assert!(observations.len() >= 3);
    assert_eq!(observations[0].0 - started, Duration::from_secs(3));
    let seen: std::collections::BTreeSet<_> = observations
        .iter()
        .flat_map(|(_, ids)| ids.iter().cloned())
        .collect();
    assert_eq!(seen.len(), 9);
    for pair in observations.windows(2) {
        assert!(pair[1].0 - pair[0].0 <= Duration::from_secs(3));
    }
    assert!(coordinator.is_ready());
    assert!(pending.iter().all(|authority| !authority.is_current()));
    coordinator.stop_admission();
    running.await.unwrap().unwrap();
}
