use super::*;

#[tokio::test]
async fn thousand_distinct_invalid_bearers_cannot_queue_behind_64_verifications() {
    let f = Fixture::new().await;
    f.control.active.store(false, Ordering::SeqCst);
    let gate = f.block().await;
    let mut admitted = Vec::new();
    for n in 0..64 {
        let auth = f.auth.clone();
        admitted.push(tokio::spawn(async move {
            identity::request_for_bearer(&auth, format!("invalid-admitted-{n}")).await
        }));
    }
    f.arrived(64).await;
    assert_eq!(f.auth.verification_slots.available_permits(), 0);
    let mut burst = tokio::task::JoinSet::new();
    for n in 0..1000 {
        let auth = f.auth.clone();
        burst.spawn(async move {
            identity::request_for_bearer(&auth, format!("invalid-burst-{n}")).await
        });
    }
    // This bound is independent of the blocked provider. No queued request
    // waits for a slot or acquires fallback authority when the pool is full.
    tokio::time::timeout(Duration::from_millis(500), async {
        while let Some(result) = burst.join_next().await {
            assert!(result.unwrap().is_err());
        }
    })
    .await
    .expect("overload must be refused before provider work");
    assert_eq!(f.control.introspection_calls.load(Ordering::SeqCst), 64);
    assert_eq!(f.auth.verification_slots.available_permits(), 0);
    gate.release.add_permits(64);
    for request in admitted {
        assert!(request.await.unwrap().unwrap().is_none());
    }
    assert_eq!(f.control.userinfo_calls.load(Ordering::SeqCst), 64);
    assert_eq!(f.control.introspection_calls.load(Ordering::SeqCst), 64);
    assert_eq!(f.auth.verification_slots.available_permits(), 64);
    assert!(f.auth.bearer_verifications.entries.lock().await.is_empty());
    *f.control.gate.write().await = None;
    f.control.active.store(true, Ordering::SeqCst);
    let valid = identity::request_for_bearer(&f.auth, "valid-after-burst".into())
        .await
        .unwrap()
        .unwrap();
    assert!(valid
        .authorization
        .allows(layer2_core::Capability::UseOwnSandboxes));
    assert_eq!(f.auth.verification_slots.available_permits(), 64);
}

#[tokio::test]
async fn cancellation_and_deadline_restore_verification_capacity() {
    let f = Fixture::new().await;
    let gate = f.block().await;
    let pending = f.verify("cancelled-verification");
    f.arrived(1).await;
    assert_eq!(f.auth.verification_slots.available_permits(), 63);
    pending.abort();
    assert!(matches!(pending.await, Err(error) if error.is_cancelled()));
    assert_eq!(f.auth.verification_slots.available_permits(), 64);
    let result = verify_with_timeout(
        &f.auth,
        &f.metadata,
        "timed-out-verification",
        Duration::from_millis(20),
    )
    .await;
    assert!(matches!(
        result,
        Err(ProviderVerificationError::Unavailable(_))
    ));
    assert_eq!(f.auth.verification_slots.available_permits(), 64);
    *f.control.gate.write().await = None;
    gate.release.add_permits(2);
    assert!(
        identity::request_for_bearer(&f.auth, "valid-after-cancellation".into())
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(f.auth.verification_slots.available_permits(), 64);
}
