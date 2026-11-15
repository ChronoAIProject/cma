use super::*;
use mongodb::{Client, Database, bson::doc};

async fn fixture() -> (Database, AuthorityStore) {
    let uri =
        std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable local replica set required");
    assert!(
        uri.starts_with("mongodb://127.0.0.1:") || uri.starts_with("mongodb://localhost:"),
        "release authority tests require an owned loopback fixture"
    );
    let client = Client::with_uri_str(uri).await.unwrap();
    let database = client.database(&format!(
        "cma_release_authority_{}",
        uuid::Uuid::new_v4().simple()
    ));
    for collection in COLLECTIONS {
        database.create_collection(*collection).await.unwrap();
    }
    let store = AuthorityStore::open(client, database.clone())
        .await
        .unwrap();
    (database, store)
}

fn intent(id: &str) -> EffectIntent {
    EffectIntent {
        effect_id: id.into(),
        resource_identity_sha256: "a".repeat(64),
        request_sha256: "b".repeat(64),
        kind: "claim".into(),
    }
}

async fn boot(store: &AuthorityStore) -> ReleaseHead {
    store.bootstrap(&bootstrap(), &TestEvidence).await.unwrap();
    store.load("test-deployment").await.unwrap().unwrap()
}

async fn advance(store: &AuthorityStore, action: AdvanceAction) -> ControlReceipt {
    let h = store.load("test-deployment").await.unwrap().unwrap();
    store
        .advance(&step(&h, action), &TestEvidence)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing to an owned local replica set"]
async fn freeze_ack_and_terminal_settlement_have_distinct_authority() {
    let (db, store) = fixture().await;
    let h = boot(&store).await;
    let EffectAdmission::New(permit) = store
        .admit_effect(&writer(1, 1), &intent("first"))
        .await
        .unwrap()
    else {
        panic!("first dispatch")
    };
    store.begin(&transition(&h), &TestEvidence).await.unwrap();
    assert_eq!(
        store
            .admit_effect(
                &writer(1, 1),
                &EffectIntent {
                    request_sha256: "c".repeat(64),
                    ..intent("second")
                }
            )
            .await,
        Err(AuthorityError::Closed)
    );
    let outcome = ObservedOutcome {
        receipt_sha256: "d".repeat(64),
        receiver_incarnation_sha256: Some("e".repeat(64)),
    };
    let observed = store
        .record_effect_outcome(&permit, &outcome)
        .await
        .unwrap();
    assert!(observed.terminal_evidence_sha256.is_none());
    let h = store.load("test-deployment").await.unwrap().unwrap();
    assert_eq!(h.active_effects, 1);
    assert_eq!(
        store
            .advance(&step(&h, AdvanceAction::ConfirmDrained), &TestEvidence)
            .await,
        Err(AuthorityError::EvidenceRequired)
    );
    let settled = store.settle_effect(&permit, &TestEvidence).await.unwrap();
    assert!(settled.terminal_evidence_sha256.is_some());
    assert_eq!(
        store.settle_effect(&permit, &TestEvidence).await.unwrap(),
        settled
    );
    assert_eq!(
        store
            .load("test-deployment")
            .await
            .unwrap()
            .unwrap()
            .active_effects,
        0
    );
    assert_eq!(
        advance(&store, AdvanceAction::ConfirmDrained).await.phase,
        Phase::Frozen
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing to an owned local replica set"]
async fn existing_identity_and_new_attempt_id_cannot_repeat_unknown_effect() {
    let (db, store) = fixture().await;
    boot(&store).await;
    let EffectAdmission::New(first) = store
        .admit_effect(&writer(1, 1), &intent("original"))
        .await
        .unwrap()
    else {
        panic!("first dispatch")
    };
    assert_eq!(
        store
            .admit_effect(&writer(1, 1), &intent("original"))
            .await
            .unwrap(),
        EffectAdmission::Existing(first.clone())
    );
    assert_eq!(
        store
            .admit_effect(&writer(1, 1), &intent("invented-retry"))
            .await
            .unwrap(),
        EffectAdmission::Existing(first.clone())
    );
    let changed = EffectIntent {
        request_sha256: "c".repeat(64),
        ..intent("original")
    };
    assert_eq!(
        store.admit_effect(&writer(1, 1), &changed).await,
        Err(AuthorityError::Conflict)
    );
    assert_eq!(
        store
            .load("test-deployment")
            .await
            .unwrap()
            .unwrap()
            .active_effects,
        1
    );
    store.settle_effect(&first, &TestEvidence).await.unwrap();
    assert!(matches!(
        store
            .admit_effect(&writer(1, 1), &intent("later-invocation"))
            .await
            .unwrap(),
        EffectAdmission::New(_)
    ));
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing to an owned local replica set"]
async fn concurrent_new_attempt_ids_admit_at_most_one_identical_effect() {
    let (db, store) = fixture().await;
    boot(&store).await;
    let w = writer(1, 1);
    let a = intent("attempt-a");
    let b = intent("attempt-b");
    let (left, right) = tokio::join!(store.admit_effect(&w, &a), store.admit_effect(&w, &b));
    let new_count = [&left, &right]
        .iter()
        .filter(|r| matches!(r, Ok(EffectAdmission::New(_))))
        .count();
    assert_eq!(new_count, 1);
    assert!(matches!(
        store.admit_effect(&w, &intent("attempt-c")).await.unwrap(),
        EffectAdmission::Existing(_)
    ));
    assert_eq!(
        store
            .load("test-deployment")
            .await
            .unwrap()
            .unwrap()
            .active_effects,
        1
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing to an owned local replica set"]
async fn freeze_racing_admission_preserves_every_granted_permit() {
    let (db, store) = fixture().await;
    let h = boot(&store).await;
    let request = transition(&h);
    let w = writer(1, 1);
    let intent = intent("racing-effect");
    let (freeze, admission) = tokio::join!(
        store.begin(&request, &TestEvidence),
        store.admit_effect(&w, &intent),
    );
    // A transaction conflict is not a grant. Reconcile the exact operation;
    // never replay a possibly acknowledged provider effect in this test.
    if freeze.is_err() {
        store.begin(&request, &TestEvidence).await.unwrap();
    }
    let h = store.load("test-deployment").await.unwrap().unwrap();
    assert_eq!(h.phase, Phase::Freezing);
    let durable = store
        .effect(&w.deployment, &intent.effect_id)
        .await
        .unwrap();
    assert_eq!(h.active_effects, i64::from(durable.is_some()));
    if let Ok(EffectAdmission::New(granted)) = admission {
        assert_eq!(durable.as_ref(), Some(&granted));
    }
    match durable {
        Some(permit) => {
            assert_eq!(
                store
                    .advance(&step(&h, AdvanceAction::ConfirmDrained), &TestEvidence)
                    .await,
                Err(AuthorityError::EvidenceRequired)
            );
            store.settle_effect(&permit, &TestEvidence).await.unwrap();
        }
        None => assert_eq!(
            store.admit_effect(&w, &intent).await,
            Err(AuthorityError::Closed)
        ),
    }
    assert_eq!(
        advance(&store, AdvanceAction::ConfirmDrained).await.phase,
        Phase::Frozen
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing to an owned local replica set"]
async fn pair_commit_replay_preserves_epoch_and_previous_writer_is_closed() {
    let (db, store) = fixture().await;
    let h = boot(&store).await;
    let request = transition(&h);
    let operation = store.begin(&request, &TestEvidence).await.unwrap();
    assert_eq!(
        store.begin(&request, &TestEvidence).await.unwrap(),
        operation
    );
    advance(&store, AdvanceAction::ConfirmDrained).await;
    advance(&store, AdvanceAction::Prepare).await;
    let before = store.load("test-deployment").await.unwrap().unwrap();
    let commit = step(&before, AdvanceAction::Commit);
    let receipt = store.advance(&commit, &TestEvidence).await.unwrap();
    assert_eq!(
        store.advance(&commit, &TestEvidence).await.unwrap(),
        receipt
    );
    let committed = store.load("test-deployment").await.unwrap().unwrap();
    assert_eq!(committed.epoch, 2);
    assert_eq!(committed.pair, request.after);
    assert_eq!(committed.writer, None);
    assert!(committed.committed_at.timestamp_millis() > 0);
    assert_eq!(
        store
            .admit_effect(&writer(1, 1), &intent("stale-writer"))
            .await,
        Err(AuthorityError::Closed)
    );
    assert_eq!(
        store
            .admit_effect(&writer(2, 2), &intent("too-early"))
            .await,
        Err(AuthorityError::Closed)
    );
    assert_eq!(
        store
            .advance(&step(&committed, AdvanceAction::Abort), &TestEvidence)
            .await,
        Err(AuthorityError::Conflict)
    );
    advance(&store, AdvanceAction::Activate).await;
    assert!(matches!(
        store
            .admit_effect(&writer(2, 2), &intent("new-writer"))
            .await
            .unwrap(),
        EffectAdmission::New(_)
    ));
    assert_eq!(
        store
            .load("test-deployment")
            .await
            .unwrap()
            .unwrap()
            .protected_pairs,
        h.protected_pairs
    );
    db.drop().await.unwrap();
}

struct RefuseEvidence;
#[async_trait::async_trait]
impl EvidenceVerifier for RefuseEvidence {
    async fn verify(&self, _: &EvidenceRequest) -> Result<VerifiedEvidence> {
        Err(AuthorityError::EvidenceRequired)
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing to an owned local replica set"]
async fn repeated_hold_keeps_one_receipt_and_abort_remains_available() {
    let (db, store) = fixture().await;
    let h = boot(&store).await;
    store.begin(&transition(&h), &TestEvidence).await.unwrap();
    let receipt = advance(&store, AdvanceAction::Hold).await;
    for _ in 0..40 {
        assert_eq!(advance(&store, AdvanceAction::Hold).await, receipt);
    }
    let op = store
        .operation("test-deployment", "restore")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(op.receipts.len(), 2);
    assert_eq!(
        advance(&store, AdvanceAction::Abort).await.phase,
        Phase::Active
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing to an owned local replica set"]
async fn full_immutable_audit_has_separate_terminal_recovery_and_replay() {
    let (db, store) = fixture().await;
    let h = boot(&store).await;
    store.begin(&transition(&h), &TestEvidence).await.unwrap();
    advance(&store, AdvanceAction::Hold).await;
    let mut op = store
        .operation("test-deployment", "restore")
        .await
        .unwrap()
        .unwrap();
    let template = op.receipts.last().unwrap().clone();
    op.receipts = (1..=32)
        .map(|revision| ControlReceipt {
            control_revision: revision,
            ..template.clone()
        })
        .collect();
    let retained = op.receipts.clone();
    db.collection::<mongodb::bson::Document>(OPERATIONS)
        .replace_one(
            doc! {"_id": &op.key},
            super::super::store::document(&op).unwrap(),
        )
        .await
        .unwrap();
    db.collection::<mongodb::bson::Document>(HEADS)
        .update_one(
            doc! {"_id": "test-deployment"},
            doc! {"$set": {"control_revision": 32_i64}},
        )
        .await
        .unwrap();
    assert_eq!(
        advance(&store, AdvanceAction::Hold).await,
        *retained.last().unwrap()
    );
    let h = store.load("test-deployment").await.unwrap().unwrap();
    let abort = step(&h, AdvanceAction::Abort);
    let recovered = store.advance(&abort, &TestEvidence).await.unwrap();
    assert_eq!(recovered.phase, Phase::Active);
    assert_eq!(
        store.advance(&abort, &TestEvidence).await.unwrap(),
        recovered
    );
    let saved = store
        .operation("test-deployment", "restore")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.receipts, retained);
    assert_eq!(saved.terminal_recovery_receipt, Some(recovered));
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing to an owned local replica set"]
async fn empty_permits_cannot_substitute_for_legacy_bootstrap_proof() {
    let (db, store) = fixture().await;
    assert_eq!(
        store.bootstrap(&bootstrap(), &RefuseEvidence).await,
        Err(AuthorityError::EvidenceRequired)
    );
    assert!(store.load("test-deployment").await.unwrap().is_none());
    assert_eq!(
        db.collection::<mongodb::bson::Document>(HEADS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        db.collection::<mongodb::bson::Document>(OPERATIONS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing to an owned local replica set"]
async fn count_corruption_cannot_certify_drain() {
    let (db, store) = fixture().await;
    let h = boot(&store).await;
    store
        .admit_effect(&writer(1, 1), &intent("unknown"))
        .await
        .unwrap();
    store.begin(&transition(&h), &TestEvidence).await.unwrap();
    db.collection::<mongodb::bson::Document>(HEADS)
        .update_one(
            doc! {"_id": "test-deployment"},
            doc! {"$set": {"active_effects": 0_i64}},
        )
        .await
        .unwrap();
    let h = store.load("test-deployment").await.unwrap().unwrap();
    assert_eq!(
        store
            .advance(&step(&h, AdvanceAction::ConfirmDrained), &TestEvidence)
            .await,
        Err(AuthorityError::EvidenceRequired)
    );
    db.drop().await.unwrap();
}
