use super::*;

pub(super) async fn retired(fixture: &Fixture) {
    tokio::time::timeout(Duration::from_secs(1), async {
        while !fixture
            .receiver
            .state
            .lock()
            .unwrap()
            .record
            .as_ref()
            .is_some_and(|record| record.retired)
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("drain must first persist its tombstone");
}

fn damage(fixture: &Fixture, fault: &str) -> Option<Vec<u8>> {
    let floor = fixture.receiver.root.join("floor.json");
    match fault {
        "missing" => std::fs::remove_file(&floor).unwrap(),
        "corrupt" => std::fs::write(&floor, b"corrupt retained custody").unwrap(),
        "changed" => {
            let mut record = persistence::load(&fixture.receiver.root).unwrap().unwrap();
            record.owner.epoch += 3;
            record.owner.session_id = "different-retained-session".into();
            std::fs::write(&floor, serde_json::to_vec(&record).unwrap()).unwrap();
        }
        "directory" => std::fs::remove_dir_all(&fixture.receiver.root).unwrap(),
        _ => panic!("unknown damage"),
    }
    assert!(fixture.receiver.root.with_extension("activated").is_file());
    std::fs::read(floor).ok()
}

async fn remains_closed(fixture: &Fixture, expected: Option<Vec<u8>>) {
    assert_eq!(
        std::fs::read(fixture.receiver.root.join("floor.json")).ok(),
        expected
    );
    assert!(fixture
        .receiver
        .register(None, |_| async { panic!("legacy effect") })
        .await
        .is_err());
    assert!(fixture
        .receiver
        .register(Some(fixture.owner(1)), |_| async { panic!("owned effect") })
        .await
        .is_err());
    assert!(fixture.receiver.tasks.lock().await.is_empty());
}

#[tokio::test(start_paused = true)]
async fn confirmation_preserves_damaged_custody_after_join_with_or_without_prior_poison() {
    for (poison, expire) in [(false, false), (true, false), (false, true), (true, true)] {
        for fault in ["missing", "corrupt", "changed", "directory"] {
            let fixture = Fixture::new();
            let (release, wait) = tokio::sync::oneshot::channel();
            fixture
                .historical_stream(|_| async move {
                    let _ = wait.await;
                })
                .await
                .unwrap();
            let challenge = fixture.receiver.challenge(fixture.request(None)).unwrap();
            let receiver = fixture.receiver.clone();
            let owner = fixture.owner(1);
            let confirm = tokio::spawn(async move {
                receiver
                    .confirm(DesktopConfirmation {
                        ownership: owner,
                        nonce: challenge.nonce,
                    })
                    .await
            });
            retired(&fixture).await;
            let expected = damage(&fixture, fault);
            if poison {
                assert!(fixture.receiver.challenge(fixture.request(None)).is_err());
            }
            if expire {
                tokio::time::advance(Duration::from_secs(11)).await;
            }
            release.send(()).unwrap();
            assert_eq!(
                confirm.await.unwrap().err().unwrap().code,
                "desktop_ownership_unavailable"
            );
            {
                let state = fixture.receiver.state.lock().unwrap();
                assert!(state.poisoned && state.legacy_closed && state.deadline.is_none());
            }
            remains_closed(&fixture, expected).await;
        }
    }
}

#[tokio::test]
async fn revoke_reports_uncertainty_when_terminal_cleanup_sees_damaged_custody() {
    for fault in ["missing", "corrupt", "changed", "directory"] {
        let fixture = Fixture::new();
        let owner = fixture.accept(1).await;
        let (release, wait) = tokio::sync::oneshot::channel();
        fixture
            .receiver
            .register(Some(owner.clone()), |_| async move {
                let _ = wait.await;
            })
            .await
            .unwrap();
        let receiver = fixture.receiver.clone();
        let revoke = tokio::spawn(async move { receiver.revoke(owner).await });
        retired(&fixture).await;
        let expected = damage(&fixture, fault);
        release.send(()).unwrap();
        assert!(revoke.await.unwrap().is_err());
        remains_closed(&fixture, expected).await;
    }
}

#[tokio::test]
async fn unpublished_temporary_floor_cannot_replace_retained_custody() {
    let fixture = Fixture::new();
    fixture.accept(1).await;
    let retained = persistence::load(&fixture.receiver.root).unwrap().unwrap();
    let temporary = fixture.receiver.root.join("interrupted.tmp");
    for bytes in [
        b"incomplete".to_vec(),
        serde_json::to_vec(&retained).unwrap(),
    ] {
        std::fs::write(&temporary, bytes).unwrap();
        assert!(persistence::load(&fixture.receiver.root).unwrap().as_ref() == Some(&retained));
        let restarted = DesktopReceiver::new(
            fixture.receiver.root.clone(),
            fixture.receiver.identity.clone(),
        );
        assert!(restarted.register(None, |_| async {}).await.is_err());
        assert!(restarted
            .challenge(fixture.request(Some(fixture.owner(1))))
            .is_err());
    }
    let next = fixture.accept(2).await;
    let published = persistence::load(&fixture.receiver.root).unwrap().unwrap();
    assert!(published.owner == next);
    assert!(!published.retired);
    assert!(temporary.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn activation_symlink_refuses_replacement_without_repairing_the_floor() {
    let fixture = Fixture::new();
    fixture.accept(1).await;
    let floor = fixture.receiver.root.join("floor.json");
    let original = std::fs::read(&floor).unwrap();
    let marker = fixture.receiver.root.with_extension("activated");
    let other = fixture.directory.join("different-marker");
    std::fs::rename(&marker, &other).unwrap();
    std::os::unix::fs::symlink(&other, &marker).unwrap();
    assert!(persistence::load(&fixture.receiver.root).is_err());
    let mut record: Record = serde_json::from_slice(&original).unwrap();
    record.owner = fixture.owner(2);
    assert!(persistence::save(&fixture.receiver.root, &record).is_err());
    assert_eq!(std::fs::read(&floor).unwrap(), original);
    assert!(marker.symlink_metadata().unwrap().file_type().is_symlink());
    assert!(fixture.receiver.challenge(fixture.request(None)).is_err());
    assert!(persistence::load(&fixture.receiver.root).is_err());
}
