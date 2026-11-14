use super::*;

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
