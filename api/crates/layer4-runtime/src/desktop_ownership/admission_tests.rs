use super::*;

#[tokio::test]
async fn successful_confirmation_bounds_custody_reads_and_publishes_each_changed_floor() {
    let f = Fixture::new();
    let mut owner = f.owner(1);
    // First admission, replacement and renewal each need the admission,
    // pre-publication and post-publication reads, without duplicate disk I/O.
    for step in 0..3 {
        let previous = (step == 2).then(|| owner.clone());
        let challenge = f.receiver.challenge(f.request(previous)).unwrap();
        if step == 1 {
            owner = f.owner(2);
        } else if step == 2 {
            owner.issued_at_ms += 1;
            owner.expires_at_ms += 1;
        }
        let before = f.receiver.custody_reads.load(Ordering::SeqCst);
        f.receiver
            .confirm(DesktopConfirmation {
                ownership: owner.clone(),
                nonce: challenge.nonce,
            })
            .await
            .unwrap();
        assert_eq!(f.receiver.custody_reads.load(Ordering::SeqCst) - before, 3);
        let retained = persistence::load(&f.receiver.root).unwrap().unwrap();
        assert!(retained.owner == owner && !retained.retired);
        assert_eq!(f.receiver.persisted.load(Ordering::SeqCst), step + 1);
    }
}

#[tokio::test(start_paused = true)]
async fn expired_confirmation_after_join_keeps_the_durable_tombstone() {
    let f = Fixture::new();
    let (release, waiting) = tokio::sync::oneshot::channel();
    f.receiver
        .register(None, |_| async move { waiting.await.unwrap() })
        .await
        .unwrap();
    let challenge = f.receiver.challenge(f.request(None)).unwrap();
    let receiver = f.receiver.clone();
    let confirmation = DesktopConfirmation {
        ownership: f.owner(1),
        nonce: challenge.nonce,
    };
    let pending = tokio::spawn(async move { receiver.confirm(confirmation).await });
    custody::retired(&f).await;
    let before = f.receiver.persisted.load(Ordering::SeqCst);
    tokio::time::advance(Duration::from_secs(11)).await;
    release.send(()).unwrap();
    assert_eq!(
        pending.await.unwrap().err().unwrap().code,
        "desktop_ownership_stale"
    );
    assert_eq!(f.receiver.persisted.load(Ordering::SeqCst), before);
    assert!(
        persistence::load(&f.receiver.root)
            .unwrap()
            .unwrap()
            .retired
    );
    assert!(!f.receiver.state.lock().unwrap().poisoned);
    assert!(f.receiver.register(None, |_| async {}).await.is_err());
    assert!(f
        .receiver
        .register(Some(f.owner(1)), |_| async {})
        .await
        .is_err());
}

#[tokio::test]
async fn empty_admission_commits_one_floor_before_ack_and_restart_refuses_replay() {
    let f = Fixture::new();
    let owner = f.accept(1).await;
    assert_eq!(f.receiver.persisted.load(Ordering::SeqCst), 1);
    let retained = persistence::load(&f.receiver.root).unwrap().unwrap();
    assert!(retained.owner == owner);
    assert!(!retained.retired);
    let restarted = DesktopReceiver::new(f.receiver.root.clone(), f.receiver.identity.clone());
    assert!(restarted.register(None, |_| async {}).await.is_err());
    assert!(restarted
        .register(Some(owner.clone()), |_| async {})
        .await
        .is_err());
    assert!(restarted.challenge(f.request(Some(owner.clone()))).is_err());
    let challenge = restarted.challenge(f.request(None)).unwrap();
    assert!(restarted
        .confirm(DesktopConfirmation {
            ownership: owner,
            nonce: challenge.nonce,
        })
        .await
        .is_err());
    assert_eq!(restarted.persisted.load(Ordering::SeqCst), 0);
    let challenge = restarted.challenge(f.request(None)).unwrap();
    restarted
        .confirm(DesktopConfirmation {
            ownership: f.owner(2),
            nonce: challenge.nonce,
        })
        .await
        .unwrap();
    assert_eq!(restarted.persisted.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn completed_custody_is_joined_before_single_write_readmission() {
    let f = Fixture::new();
    let owner = f.accept(1).await;
    let (done, finished) = tokio::sync::oneshot::channel();
    f.receiver
        .register(Some(owner), |_| async move { done.send(()).unwrap() })
        .await
        .unwrap();
    finished.await.unwrap();
    // Await actual completion without consuming the retained join handle.
    // An occupied JoinSet is not proof that any old I/O remains live.
    tokio::task::yield_now().await;
    assert_eq!(f.receiver.tasks.lock().await.len(), 1);
    assert!(
        f.receiver
            .state
            .lock()
            .unwrap()
            .record
            .as_ref()
            .unwrap()
            .retired
    );
    let before = f.receiver.persisted.load(Ordering::SeqCst);
    f.accept(2).await;
    assert!(f.receiver.tasks.lock().await.is_empty());
    assert_eq!(f.receiver.persisted.load(Ordering::SeqCst) - before, 1);
}

#[tokio::test]
async fn pending_custody_requires_tombstone_before_join_and_active_floor_after_join() {
    let f = Fixture::new();
    let (release, waiting) = tokio::sync::oneshot::channel();
    f.receiver
        .register(None, |_| async move { waiting.await.unwrap() })
        .await
        .unwrap();
    let challenge = f.receiver.challenge(f.request(None)).unwrap();
    let receiver = f.receiver.clone();
    let confirmation = DesktopConfirmation {
        ownership: f.owner(1),
        nonce: challenge.nonce,
    };
    let pending = tokio::spawn(async move { receiver.confirm(confirmation).await });
    custody::retired(&f).await;
    assert_eq!(f.receiver.persisted.load(Ordering::SeqCst), 1);
    assert!(
        persistence::load(&f.receiver.root)
            .unwrap()
            .unwrap()
            .retired
    );
    assert!(!pending.is_finished());
    release.send(()).unwrap();
    pending.await.unwrap().unwrap();
    assert_eq!(f.receiver.persisted.load(Ordering::SeqCst), 2);
    assert!(
        !persistence::load(&f.receiver.root)
            .unwrap()
            .unwrap()
            .retired
    );
}

#[tokio::test]
async fn revoke_and_task_completion_share_one_durable_retirement() {
    let f = Fixture::new();
    let owner = f.accept(1).await;
    let (started, waiting) = tokio::sync::oneshot::channel();
    f.receiver
        .register(Some(owner.clone()), move |session| async move {
            started.send(()).unwrap();
            assert!(session.run(std::future::pending::<()>()).await.is_err());
        })
        .await
        .unwrap();
    waiting.await.unwrap();
    let before = f.receiver.persisted.load(Ordering::SeqCst);
    f.receiver.revoke(owner.clone()).await.unwrap();
    assert!(f.receiver.tasks.lock().await.is_empty());
    assert_eq!(f.receiver.persisted.load(Ordering::SeqCst) - before, 1);
    let retired = persistence::load(&f.receiver.root).unwrap().unwrap();
    assert!(retired.retired && retired.owner == owner);
    f.receiver.revoke(owner).await.unwrap();
    assert_eq!(f.receiver.persisted.load(Ordering::SeqCst) - before, 1);
    assert!(f.receiver.register(None, |_| async {}).await.is_err());
}

#[tokio::test]
async fn unchanged_record_still_requires_current_durable_custody() {
    for missing in [false, true] {
        let f = Fixture::new();
        let owner = f.accept(1).await;
        f.receiver.revoke(owner).await.unwrap();
        let before = f.receiver.persisted.load(Ordering::SeqCst);
        let floor = f.receiver.root.join("floor.json");
        if missing {
            std::fs::remove_file(&floor).unwrap();
        } else {
            std::fs::write(&floor, b"damaged custody").unwrap();
        }
        let mut state = f.receiver.state.lock().unwrap();
        let unchanged = state.record.clone().unwrap();
        assert!(f.receiver.persist(&mut state, unchanged).is_err());
        assert!(state.poisoned && state.legacy_closed && state.deadline.is_none());
        assert_eq!(f.receiver.persisted.load(Ordering::SeqCst), before);
        assert!(persistence::load(&f.receiver.root).is_err());
    }
}
