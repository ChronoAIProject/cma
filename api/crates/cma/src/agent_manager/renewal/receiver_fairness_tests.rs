use super::*;
use layer3_adapter::owner_protocol::TransportAuthority;

#[tokio::test(start_paused = true)]
async fn overwritten_renewal_updates_do_not_starve_cancelled_receiver_cleanup() {
    let store = DelayedStore::new(Duration::from_millis(25));
    let coordinator = coordinator(store.clone()).await;
    let healthy = coordinator
        .acquire(&AgentId("healthy".into()))
        .await
        .unwrap()
        .unwrap();
    let mut cancelled = Vec::new();
    for index in 0..8 {
        let authority = coordinator
            .acquire(&AgentId(format!("cancelled-{index}")))
            .await
            .unwrap()
            .unwrap();
        TransportAuthority::revoke(authority.as_ref());
        cancelled.push(authority);
    }
    let attempted = Arc::new(Mutex::new(BTreeMap::<AgentId, usize>::new()));
    let consumed = Arc::new(AtomicUsize::new(0));
    let running = tokio::spawn(run(coordinator.clone(), {
        let coordinator = coordinator.clone();
        let attempted = attempted.clone();
        let consumed = consumed.clone();
        move |_| {
            let coordinator = coordinator.clone();
            let attempted = attempted.clone();
            let consumed = consumed.clone();
            async move {
                consumed.fetch_add(1, Ordering::Relaxed);
                // The production consumer selects this batch when it can
                // actually process it, not on each overwritten producer tick.
                let batch = coordinator.cancelled_receivers().await;
                assert_eq!(batch.len(), 4);
                for authority in batch {
                    *attempted
                        .lock()
                        .unwrap()
                        .entry(authority.token.agent_id.clone())
                        .or_default() += 1;
                    // A missing receiver ACK consumes the bounded store/receiver
                    // deadline; it cannot release this exact durable owner.
                    assert!(coordinator
                        .handoff(&authority, std::future::pending())
                        .await
                        .is_err());
                }
                // Four timeouts plus this delay span four renewal ticks. A
                // producer-only modulo8 cursor can alias the same cohort forever.
                tokio::time::sleep(Duration::from_secs(4)).await;
                Ok(())
            }
        }
    }));
    tokio::time::sleep(Duration::from_secs(35)).await;
    assert!(coordinator.is_ready());
    assert!(healthy.is_current());
    assert!(!*coordinator.failure_signal().borrow());
    assert!(consumed.load(Ordering::Relaxed) >= 3);
    assert_eq!(
        attempted.lock().unwrap().len(),
        8,
        "slow cleanup must attempt both cancelled cohorts despite overwritten watch updates"
    );
    assert!(cancelled.iter().all(|authority| !authority.is_current()));
    healthy
        .run(store.check(&healthy.token))
        .await
        .unwrap()
        .unwrap();
    coordinator.stop_admission();
    running.await.unwrap().unwrap();
}
