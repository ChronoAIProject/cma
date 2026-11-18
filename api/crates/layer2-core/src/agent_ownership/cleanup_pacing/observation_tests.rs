use super::*;

#[tokio::test(start_paused = true)]
async fn observation_capacity_and_expiry_never_discard_inflight_attempts() {
    let pacing = CleanupPacing::new(true, 2);
    let mut authorities = Vec::new();
    for index in 0..4 {
        let authority = Authority::new(
            OwnershipToken {
                agent_id: AgentId(format!("agent-{index}")),
                instance_id: "owner".into(),
                epoch: 1,
                protocol: PROTOCOL_VERSION,
            },
            tokio::time::Instant::now() + Duration::from_secs(100),
        );
        authority
            .register_receiver(Receiver::new("binding"))
            .unwrap();
        authorities.push(authority);
    }
    let a = pacing.begin(&authorities[0]).unwrap();
    let b = pacing.begin(&authorities[1]).unwrap();
    tokio::time::advance(Duration::from_secs(600)).await;
    assert!(pacing.begin(&authorities[2]).is_none());
    assert!(pacing.deferred(&authorities[0].token.agent_id));
    drop(a);
    drop(b);
    for authority in &authorities[2..] {
        drop(pacing.begin(authority).unwrap());
    }
    let state = pacing.state.lock().unwrap();
    assert!(state.active.is_empty());
    assert_eq!(state.bindings.len(), 2);
    drop(state);
    // Evicted history permits a retry, but cannot finish any ownership handoff.
    assert!(pacing.begin(&authorities[0]).is_some());
}

#[tokio::test(start_paused = true)]
async fn verified_replacement_clears_only_inactive_matching_agent_history() {
    let pacing = CleanupPacing::new(true, 2);
    let a = observation(1, "binding-a", true);
    let b = observation(2, "binding-b", false);
    let active = pacing.begin(&a).unwrap();
    b.receiver_confirmed("binding-b");
    pacing.verified_replacement(&b);
    assert!(pacing.deferred(&a.token.agent_id));
    drop(active);
    pacing.verified_replacement(&b);
    assert!(!pacing.deferred(&a.token.agent_id));
    drop(pacing.begin(&b).unwrap());
    pacing.verified_replacement(&a);
    assert!(
        pacing.begin(&a).is_none(),
        "a delayed older verified binding cannot reset newer failure history"
    );
    pacing.verified_replacement(&observation(3, "binding-c", false));
    assert!(pacing.deferred(&b.token.agent_id));
}

#[tokio::test]
async fn arbitrary_or_cancelled_confirmation_never_verifies_a_receiver() {
    let a = observation(1, "binding-a", false);
    a.receiver_confirmed("different-binding");
    assert!(!a.receiver_binding().unwrap().1);
    a.revoke();
    a.receiver_confirmed("binding-a");
    assert!(!a.receiver_binding().unwrap().1);
    let bare = Authority::new(
        a.token.clone(),
        tokio::time::Instant::now() + Duration::from_secs(1),
    );
    assert_eq!(bare.receiver_binding(), None);
}

#[tokio::test(start_paused = true)]
async fn repeated_same_binding_failures_keep_escalating_across_epochs() {
    let pacing = CleanupPacing::new(true, 2);
    for epoch in 1..=12 {
        let authority = observation(epoch, "binding-a", false);
        drop(pacing.begin(&authority).unwrap());
        let expected = Duration::from_secs((3_u64 << (epoch - 1)).min(60));
        let remaining = pacing
            .state
            .lock()
            .unwrap()
            .schedule
            .remaining(&authority.token.agent_id);
        assert!(remaining >= expected && remaining <= Duration::from_secs(60));
        tokio::time::advance(remaining).await;
    }
    assert_eq!(pacing.state.lock().unwrap().bindings.len(), 1);
}

#[tokio::test(start_paused = true)]
async fn rotating_cancelled_batches_include_cooled_and_new_failures_fairly() {
    let (_, coordinator, first, _) = fixture(true).await;
    TransportAuthority::revoke(first.as_ref());
    let mut ids = vec![first.token.agent_id.clone()];
    for index in 1..8 {
        let a = coordinator
            .acquire(&AgentId(format!("cancelled-{index}")))
            .await
            .unwrap()
            .unwrap();
        a.register_receiver(Receiver::new("binding")).unwrap();
        TransportAuthority::revoke(a.as_ref());
        ids.push(a.token.agent_id.clone());
    }
    let mut visited = std::collections::BTreeSet::new();
    for _ in 0..2 {
        for authority in coordinator.cancelled_receivers().await {
            visited.insert(authority.token.agent_id.clone());
            assert!(coordinator.handoff_background(&authority).await.is_err());
        }
    }
    assert_eq!(visited.len(), ids.len());
    for id in ids {
        assert!(coordinator.background_cleanup_deferred(&id).await);
    }
}
