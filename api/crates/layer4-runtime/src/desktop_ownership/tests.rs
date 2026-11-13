use super::*;
use layer3_adapter::desktop_ownership::wire::DesktopConfirmation;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

struct Fixture {
    directory: PathBuf,
    receiver: Arc<DesktopReceiver>,
}
impl Fixture {
    fn new() -> Self {
        let directory =
            std::env::temp_dir().join(format!("cma-desktop-owner-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let receiver = DesktopReceiver::new(
            directory.join("desktop-owner"),
            Identity {
                sandbox: "sandbox".into(),
                generation: 3,
                home: Some("assigned-home".into()),
                boot: "boot-one".into(),
            },
        );
        Self {
            directory,
            receiver,
        }
    }
    fn owner(&self, epoch: u64) -> DesktopOwnership {
        DesktopOwnership {
            sandbox_id: "sandbox".into(),
            runtime_id: "external-handle".into(),
            boot_id: None,
            session_id: format!("session-{epoch}"),
            epoch,
            issued_at_ms: 1000,
            expires_at_ms: 16000,
        }
    }
    fn request(&self, previous: Option<DesktopOwnership>) -> DesktopChallengeRequest {
        DesktopChallengeRequest {
            sandbox_id: "sandbox".into(),
            runtime_id: "external-handle".into(),
            previous,
        }
    }
    async fn accept(&self, epoch: u64) -> DesktopOwnership {
        let owner = self.owner(epoch);
        let challenge = self.receiver.challenge(self.request(None)).unwrap();
        self.receiver
            .confirm(DesktopConfirmation {
                ownership: owner.clone(),
                nonce: challenge.nonce,
            })
            .await
            .unwrap();
        owner
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

#[tokio::test(start_paused = true)]
async fn expiry_and_revocation_cancel_blocked_sends_before_acknowledgement() {
    for expire in [false, true] {
        let f = Fixture::new();
        let owner = f.accept(1).await;
        let (sink, mut source) = tokio::sync::mpsc::channel(1);
        sink.send(1).await.unwrap();
        let (started, waiting) = tokio::sync::oneshot::channel();
        let (done, finished) = tokio::sync::oneshot::channel();
        f.receiver
            .register(Some(owner.clone()), move |session| async move {
                started.send(()).unwrap();
                assert!(session.run(sink.send(2)).await.is_err());
                done.send(()).unwrap();
            })
            .await
            .unwrap();
        waiting.await.unwrap();
        if expire {
            tokio::time::advance(Duration::from_secs(11)).await;
        }
        f.receiver.revoke(owner).await.unwrap();
        finished.await.unwrap();
        assert_eq!(source.recv().await, Some(1));
        assert_eq!(source.recv().await, None);
    }
}

#[tokio::test]
async fn competing_confirmations_consume_one_nonce_and_lost_ack_keeps_floor() {
    let f = Fixture::new();
    let challenge = f.receiver.challenge(f.request(None)).unwrap();
    let confirmation = DesktopConfirmation {
        ownership: f.owner(1),
        nonce: challenge.nonce,
    };
    let (left, right) = tokio::join!(
        f.receiver.confirm(confirmation.clone()),
        f.receiver.confirm(confirmation)
    );
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    // Drop the acknowledgement as if its transport response was lost.
    drop((left, right));
    let reissued = f.receiver.challenge(f.request(None)).unwrap();
    assert!(f
        .receiver
        .confirm(DesktopConfirmation {
            ownership: f.owner(1),
            nonce: reissued.nonce
        })
        .await
        .is_err());
    assert!(f.receiver.register(None, |_| async {}).await.is_err());
}

#[tokio::test]
async fn exact_binding_single_use_and_persistent_floor_survive_new_boot() {
    let f = Fixture::new();
    let mut wrong = f.request(None);
    wrong.sandbox_id = "unrelated-pool-sandbox".into();
    assert!(f.receiver.challenge(wrong).is_err());
    for changed in ["runtime", "sandbox", "boot"] {
        let challenge = f.receiver.challenge(f.request(None)).unwrap();
        let mut owner = f.owner(1);
        match changed {
            "runtime" => owner.runtime_id = "replacement".into(),
            "sandbox" => owner.sandbox_id = "other".into(),
            _ => owner.boot_id = Some("other-boot".into()),
        }
        assert!(f
            .receiver
            .confirm(DesktopConfirmation {
                ownership: owner,
                nonce: challenge.nonce
            })
            .await
            .is_err());
    }
    let challenge = f.receiver.challenge(f.request(None)).unwrap();
    let confirmation = DesktopConfirmation {
        ownership: f.owner(1),
        nonce: challenge.nonce,
    };
    f.receiver.confirm(confirmation.clone()).await.unwrap();
    assert!(f.receiver.confirm(confirmation).await.is_err());
    let mut identity = f.receiver.identity.clone();
    identity.boot = "boot-two".into();
    let restarted = DesktopReceiver::new(f.receiver.root.clone(), identity);
    assert!(restarted.register(None, |_| async {}).await.is_err());
    assert!(restarted.challenge(f.request(Some(f.owner(1)))).is_err());
    let challenge = restarted.challenge(f.request(None)).unwrap();
    assert!(restarted
        .confirm(DesktopConfirmation {
            ownership: f.owner(1),
            nonce: challenge.nonce
        })
        .await
        .is_err());
    let challenge = restarted.challenge(f.request(None)).unwrap();
    restarted
        .confirm(DesktopConfirmation {
            ownership: f.owner(2),
            nonce: challenge.nonce,
        })
        .await
        .unwrap();
}

#[tokio::test(start_paused = true)]
async fn expiry_refuses_ready_frames_late_confirmation_and_revoked_session_renewal() {
    let f = Fixture::new();
    let owner = f.accept(1).await;
    let session = Session {
        receiver: f.receiver.clone(),
        revision: 1,
        owner: Some(owner.clone()),
    };
    let challenge = f
        .receiver
        .challenge(f.request(Some(owner.clone())))
        .unwrap();
    tokio::time::advance(Duration::from_secs(11)).await;
    let polled = AtomicUsize::new(0);
    assert!(session
        .run(async {
            polled.fetch_add(1, Ordering::SeqCst);
        })
        .await
        .is_err());
    assert_eq!(polled.load(Ordering::SeqCst), 0);
    assert!(f
        .receiver
        .confirm(DesktopConfirmation {
            ownership: owner.clone(),
            nonce: challenge.nonce
        })
        .await
        .is_err());
    assert!(f
        .receiver
        .challenge(f.request(Some(owner.clone())))
        .is_err());
    f.receiver.revoke(owner.clone()).await.unwrap();
    assert!(f.receiver.challenge(f.request(Some(owner))).is_err());
    assert!(f.receiver.register(None, |_| async {}).await.is_err());
}

#[tokio::test]
async fn first_owned_admission_cancels_and_joins_pending_legacy_upgrade() {
    let f = Fixture::new();
    let (ready, received) = tokio::sync::oneshot::channel();
    let finished = Arc::new(AtomicUsize::new(0));
    let count = finished.clone();
    f.receiver
        .register(None, move |session| async move {
            ready.send(()).unwrap();
            assert!(session.run(std::future::pending::<()>()).await.is_err());
            count.fetch_add(1, Ordering::SeqCst);
        })
        .await
        .unwrap();
    received.await.unwrap();
    let owner = f.accept(1).await;
    assert_eq!(finished.load(Ordering::SeqCst), 1);
    assert!(f.receiver.register(None, |_| async {}).await.is_err());
    let (ready, received) = tokio::sync::oneshot::channel();
    let count = finished.clone();
    f.receiver
        .register(Some(owner.clone()), move |session| async move {
            ready.send(()).unwrap();
            assert!(session.run(std::future::pending::<()>()).await.is_err());
            count.fetch_add(1, Ordering::SeqCst);
        })
        .await
        .unwrap();
    received.await.unwrap();
    assert!(f
        .receiver
        .register(Some(owner.clone()), |_| async {})
        .await
        .is_err());
    f.receiver.revoke(owner.clone()).await.unwrap();
    assert_eq!(finished.load(Ordering::SeqCst), 2);
    assert!(f.receiver.challenge(f.request(Some(owner))).is_err());
}

#[tokio::test]
async fn cancelled_takeover_keeps_task_custody_and_retired_epoch() {
    let f = Fixture::new();
    let (release, wait) = tokio::sync::oneshot::channel();
    f.receiver
        .register(None, |_| async move {
            let _ = wait.await;
        })
        .await
        .unwrap();
    let challenge = f.receiver.challenge(f.request(None)).unwrap();
    let receiver = f.receiver.clone();
    let owner = f.owner(1);
    let takeover = tokio::spawn(async move {
        receiver
            .confirm(DesktopConfirmation {
                ownership: owner,
                nonce: challenge.nonce,
            })
            .await
    });
    while !f.receiver.state.lock().unwrap().legacy_closed {
        tokio::task::yield_now().await;
    }
    takeover.abort();
    assert!(matches!(takeover.await, Err(error) if error.is_cancelled()));
    assert_eq!(f.receiver.tasks.lock().await.len(), 1);
    assert!(f.receiver.register(None, |_| async {}).await.is_err());
    release.send(()).unwrap();
    f.accept(2).await;
    assert!(f.receiver.tasks.lock().await.is_empty());
}

#[tokio::test]
async fn persistence_failure_corruption_and_missing_floor_never_restore_legacy() {
    for damage in ["missing", "corrupt", "write", "directory"] {
        let f = Fixture::new();
        let owner = f.accept(1).await;
        let floor = f.receiver.root.join("floor.json");
        match damage {
            "directory" => std::fs::remove_dir_all(&f.receiver.root).unwrap(),
            "missing" => std::fs::remove_file(&floor).unwrap(),
            "corrupt" => std::fs::write(&floor, b"invalid").unwrap(),
            _ => {
                std::fs::remove_file(&floor).unwrap();
                std::fs::create_dir(&floor).unwrap();
            }
        }
        assert!(f.receiver.revoke(owner).await.is_err());
        let restarted = DesktopReceiver::new(f.receiver.root.clone(), f.receiver.identity.clone());
        assert!(restarted.challenge(f.request(None)).is_err());
        assert!(restarted.register(None, |_| async {}).await.is_err());
    }
    let f = Fixture::new();
    let challenge = f.receiver.challenge(f.request(None)).unwrap();
    std::fs::create_dir(&f.receiver.root).unwrap();
    std::fs::create_dir(f.receiver.root.with_extension("activated")).unwrap();
    assert!(f
        .receiver
        .confirm(DesktopConfirmation {
            ownership: f.owner(1),
            nonce: challenge.nonce
        })
        .await
        .is_err());
    assert!(f.receiver.register(None, |_| async {}).await.is_err());
}

#[tokio::test(start_paused = true)]
async fn renewal_keeps_original_challenge_deadline_and_completion_retires_session() {
    let f = Fixture::new();
    let owner = f.accept(1).await;
    tokio::time::advance(Duration::from_secs(3)).await;
    let challenge = f
        .receiver
        .challenge(f.request(Some(owner.clone())))
        .unwrap();
    let expected = Instant::now() + Duration::from_secs(10);
    tokio::time::advance(Duration::from_secs(2)).await;
    let mut next = owner.clone();
    next.issued_at_ms += 3000;
    next.expires_at_ms += 3000;
    f.receiver
        .confirm(DesktopConfirmation {
            ownership: next,
            nonce: challenge.nonce,
        })
        .await
        .unwrap();
    assert_eq!(f.receiver.state.lock().unwrap().deadline, Some(expected));
    let (done, received) = tokio::sync::oneshot::channel();
    f.receiver
        .register(Some(owner.clone()), |_| async move {
            let _ = done.send(());
        })
        .await
        .unwrap();
    received.await.unwrap();
    while !f.receiver.tasks.lock().await.is_empty() {
        tokio::task::yield_now().await;
        f.receiver.tasks.lock().await.try_join_next();
    }
    assert!(f.receiver.challenge(f.request(Some(owner))).is_err());
}
