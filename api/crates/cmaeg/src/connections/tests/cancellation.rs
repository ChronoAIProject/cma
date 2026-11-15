use super::{outbox::RecordingLauncher, *};

#[tokio::test]
async fn cancellation_recovers_missing_or_unrelated_channel_reservations_and_releases_exact_claim()
{
    for unrelated in [false, true] {
        let fixture = Fixture::new().await;
        let service = fixture.service();
        let mut record = connection("alice", false);
        record.status = Status::Ready;
        record.channel_record = Some(uuid::Uuid::new_v4().to_string());
        if unrelated {
            retain_bound_channel(&fixture, &record, Some("a-different-connection"));
        }
        fixture
            .store()
            .save_connection(&record, None)
            .await
            .unwrap();
        fixture
            .store()
            .claim_bot("9901", "alice", &record.id)
            .await
            .unwrap();
        assert!(!service.channel_exists(&record).await.unwrap());
        service
            .cancel(&owner("alice", false), &record.id)
            .await
            .unwrap();
        let (cancelled, _) = fixture.store().connection(&record.id).await.unwrap();
        assert_eq!(cancelled.status, Status::Cancelled);
        assert!(cancelled.channel_record.is_none());
        assert!(cancelled.channel_id.is_none());
        assert!(cancelled.browser_nonce.is_none());
        let replacement = uuid::Uuid::new_v4().to_string();
        fixture
            .store()
            .claim_bot("9901", "alice", &replacement)
            .await
            .unwrap();
        // A retry of the previous cancellation must not release a newer claim.
        service
            .cancel(&owner("alice", false), &record.id)
            .await
            .unwrap();
        assert!(matches!(
            fixture
                .store()
                .claim_bot("9901", "bob", &uuid::Uuid::new_v4().to_string())
                .await,
            Err(Error::Conflict)
        ));
        let (claim, _) = fixture
            .store()
            .load(&index_id("platform-bot", &["telegram", "9901"]))
            .await
            .unwrap();
        assert!(
            matches!(claim,Stored::BotClaim(value) if value.connection_id==replacement && value.owner=="alice")
        );
    }
}

#[tokio::test]
async fn lost_claim_release_response_can_be_retried_without_releasing_new_owner_claim() {
    let fixture = Fixture::new().await;
    let service = fixture.service();
    let record = connection("alice", false);
    fixture
        .store()
        .save_connection(&record, None)
        .await
        .unwrap();
    fixture
        .store()
        .claim_bot("9901", "alice", &record.id)
        .await
        .unwrap();
    *fixture.peer.lose_next_put_for.lock().unwrap() =
        Some(index_id("platform-bot", &["telegram", "9901"]));
    assert!(matches!(
        service.cancel(&owner("alice", false), &record.id).await,
        Err(Error::Unavailable)
    ));
    assert_eq!(
        fixture
            .store()
            .connection(&record.id)
            .await
            .unwrap()
            .0
            .status,
        Status::Cancelled
    );
    let next = uuid::Uuid::new_v4().to_string();
    fixture
        .store()
        .claim_bot("9901", "bob", &next)
        .await
        .unwrap();
    fixture
        .service()
        .cancel(&owner("alice", false), &record.id)
        .await
        .unwrap();
    fixture
        .store()
        .claim_bot("9901", "bob", &next)
        .await
        .unwrap();
    assert!(matches!(
        fixture.store().claim_bot("9901", "alice", &record.id).await,
        Err(Error::Conflict)
    ));
}

#[tokio::test]
async fn delayed_claim_release_cannot_erase_a_reacquired_claim() {
    let fixture = Fixture::new().await;
    let first = uuid::Uuid::new_v4().to_string();
    let next = uuid::Uuid::new_v4().to_string();
    fixture
        .store()
        .claim_bot("9901", "alice", &first)
        .await
        .unwrap();
    fixture
        .peer
        .pause_next_mutation
        .store(true, Ordering::SeqCst);
    let delayed_store = fixture.store();
    let delayed_id = first.clone();
    let delayed = tokio::spawn(async move {
        delayed_store
            .release_bot_claim("9901", "alice", &delayed_id)
            .await
    });
    tokio::time::timeout(
        Duration::from_secs(3),
        fixture.peer.mutation_started.notified(),
    )
    .await
    .unwrap();
    // A second replica releases Alice's same claim; Bob then acquires the slot.
    fixture
        .store()
        .release_bot_claim("9901", "alice", &first)
        .await
        .unwrap();
    fixture
        .store()
        .claim_bot("9901", "bob", &next)
        .await
        .unwrap();
    fixture.peer.resume_mutation.notify_one();
    let _ = tokio::time::timeout(Duration::from_secs(3), delayed)
        .await
        .unwrap()
        .unwrap();
    fixture
        .store()
        .claim_bot("9901", "bob", &next)
        .await
        .unwrap();
    assert!(matches!(
        fixture.store().claim_bot("9901", "alice", &first).await,
        Err(Error::Conflict)
    ));
    let (stored, version) = fixture
        .store()
        .load(&index_id("platform-bot", &["telegram", "9901"]))
        .await
        .unwrap();
    assert!(
        version >= 3,
        "releasing and reclaiming must preserve monotonic revisions"
    );
    assert!(
        matches!(stored,Stored::BotClaim(claim) if claim.owner=="bob" && claim.connection_id==next)
    );
}

#[tokio::test]
async fn cancellation_between_material_save_and_claim_releases_the_late_claim() {
    let fixture = Fixture::new().await;
    let mut record = connection("alice", false);
    record.material = None;
    fixture
        .store()
        .save_connection(&record, None)
        .await
        .unwrap();
    let claim_id = index_id("platform-bot", &["telegram", "9901"]);
    *fixture.peer.pause_next_mutation_for.lock().unwrap() = Some(claim_id.clone());
    let completing = fixture.service();
    let input = record.clone();
    let completion = tokio::spawn(async move {
        completing
            .complete(
                input,
                1,
                crate::telegram::BotIdentity {
                    id: "9901".into(),
                    username: Some("paper_fixture_bot".into()),
                    first_name: "Paper assistant".into(),
                    can_manage_bots: false,
                },
                Secret::new(CHILD_TOKEN),
            )
            .await
    });
    tokio::time::timeout(
        Duration::from_secs(3),
        fixture.peer.mutation_started.notified(),
    )
    .await
    .unwrap();
    let (persisted, version) = fixture.store().connection(&record.id).await.unwrap();
    assert_eq!(version, 2);
    assert_eq!(persisted.material.as_ref().unwrap().token, CHILD_TOKEN);
    assert!(matches!(
        fixture.store().load(&claim_id).await,
        Err(Error::NotFound)
    ));
    fixture
        .service()
        .cancel(&owner("alice", false), &record.id)
        .await
        .unwrap();
    fixture.peer.resume_mutation.notify_one();
    let result = tokio::time::timeout(Duration::from_secs(3), completion)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(result, Err(Error::Conflict)));
    let (cancelled, _) = fixture.store().connection(&record.id).await.unwrap();
    assert_eq!(cancelled.status, Status::Cancelled);
    assert!(cancelled.browser_nonce.is_none());
    let (claim, _) = fixture.store().load(&claim_id).await.unwrap();
    assert!(matches!(claim,Stored::BotClaim(value) if value.released));
    fixture
        .store()
        .claim_bot("9901", "bob", &uuid::Uuid::new_v4().to_string())
        .await
        .unwrap();
}

#[tokio::test]
async fn terminal_claim_cleanup_retries_after_lost_response_without_releasing_replacement_claim() {
    for terminal in [Status::Cancelled, Status::Expired] {
        let fixture = Fixture::new().await;
        let mut record = connection("alice", false);
        record.status = if terminal == Status::Expired {
            Status::AwaitingPlatform
        } else {
            terminal
        };
        if terminal == Status::Expired {
            record.expires_at = now() - 1;
        }
        fixture
            .store()
            .save_connection(&record, None)
            .await
            .unwrap();
        fixture
            .store()
            .claim_bot("9901", "alice", &record.id)
            .await
            .unwrap();
        let claim_id = index_id("platform-bot", &["telegram", "9901"]);
        *fixture.peer.lose_next_put_for.lock().unwrap() = Some(claim_id.clone());
        // The real worker must reconcile terminal records even though they
        // never qualify for a completion notification or platform refresh.
        let launcher = Arc::new(RecordingLauncher::default());
        let worker = fixture.service().start_completion_worker(launcher.clone());
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let (stored, _) = fixture.store().load(&claim_id).await.unwrap();
                if matches!(stored,Stored::BotClaim(value) if value.released) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        drop(worker);
        assert!(fixture.peer.lose_next_put_for.lock().unwrap().is_none());
        assert!(launcher.requests.lock().unwrap().is_empty());
        let replacement = uuid::Uuid::new_v4().to_string();
        fixture
            .store()
            .claim_bot("9901", "bob", &replacement)
            .await
            .unwrap();
        // Restart with only the durable record, as the worker's next scan does.
        let restarted = fixture.service();
        let (retained, _) = restarted.store.connection(&record.id).await.unwrap();
        restarted.reconcile_terminal_claim(&retained).await.unwrap();
        restarted.reconcile_terminal_claim(&retained).await.unwrap();
        fixture
            .store()
            .claim_bot("9901", "bob", &replacement)
            .await
            .unwrap();
        let (claim, _) = fixture.store().load(&claim_id).await.unwrap();
        assert!(
            matches!(claim,Stored::BotClaim(value) if !value.released && value.owner=="bob" && value.connection_id==replacement)
        );
    }
}

#[tokio::test]
async fn background_recovery_does_not_release_a_reservation_before_activation_was_persisted() {
    let fixture = Fixture::new().await;
    let mut record = connection("alice", false);
    record.status = Status::Ready;
    record.channel_record = Some(uuid::Uuid::new_v4().to_string());
    assert!(record.channel_id.is_none());
    fixture
        .store()
        .save_connection(&record, None)
        .await
        .unwrap();
    fixture
        .service()
        .recover_deleted_channel(record.clone(), 1)
        .await
        .unwrap();
    let (retained, version) = fixture.store().connection(&record.id).await.unwrap();
    assert_eq!(version, 1);
    assert_eq!(retained.channel_record, record.channel_record);
    assert_eq!(retained.status, Status::Ready);
}
