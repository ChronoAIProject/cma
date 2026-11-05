use super::*;
use crate::store::agent_log_segments::publication::PublicationIdentity;
use crate::store::agent_log_segments::{writer_admission as admission, writer_gate as gate};
use crate::store::sandbox_mongo::{commit_transaction, start_transaction, CommitOutcome};

async fn transaction(client: &Client) -> mongodb::ClientSession {
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    session
}

async fn commit(session: &mut mongodb::ClientSession) {
    assert!(matches!(
        commit_transaction(session).await,
        CommitOutcome::Committed
    ));
}

async fn fixture(prefix: &str) -> (Client, String, MongoSandboxStore, Agent) {
    let (client, name) = database(prefix).await;
    let db = client.database(&name);
    segments::migrate(&db).await.unwrap();
    db.create_collection(gate::COLLECTION).await.unwrap();
    db.create_collection(admission::COLLECTION).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (_, agent) = worktree_and_agent(&SandboxId("sbx_gate_fixture".into()));
    store.create_agent(agent.clone()).await.unwrap();
    (client, name, store, agent)
}

fn selected(agent: &Agent) -> gate::Policy {
    gate::Policy {
        enabled: true,
        agents: vec![agent.id.0.clone()],
        owners: Vec::new(),
    }
}

fn operation() -> PublicationIdentity {
    PublicationIdentity {
        key: "fixture-publication".into(),
        fingerprint: segments::publication::result_digest(&("fixture-request", 1_i64)).unwrap(),
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn gates_default_closed_and_stale_snapshot_cannot_admit_after_closure() {
    let (client, name, _, agent) = fixture("segment_gate_race").await;
    let db = client.database(&name);
    let mut session = transaction(&client).await;
    assert!(
        gate::select_and_fence(&db, gate::GateName::Segments, &agent, &mut session)
            .await
            .unwrap()
            .is_none()
    );
    session.abort_transaction().await.unwrap();
    let mut session = transaction(&client).await;
    gate::replace(
        &db,
        gate::GateName::Segments,
        0,
        selected(&agent),
        &mut session,
    )
    .await
    .unwrap();
    commit(&mut session).await;

    let mut stale = transaction(&client).await;
    assert!(
        gate::read(&db, gate::GateName::Segments, &mut stale)
            .await
            .unwrap()
            .unwrap()
            .policy
            .enabled
    );
    let mut closing = transaction(&client).await;
    gate::replace(
        &db,
        gate::GateName::Segments,
        1,
        gate::Policy::default(),
        &mut closing,
    )
    .await
    .unwrap();
    commit(&mut closing).await;
    assert!(matches!(
        gate::select_and_fence(&db, gate::GateName::Segments, &agent, &mut stale).await,
        Err(crate::store::agent_log_integrity::TransactionLogError::Driver(error))
            if error.contains_label("TransientTransactionError")
    ));
    let _ = stale.abort_transaction().await;
    let mut session = transaction(&client).await;
    assert!(
        gate::select_and_fence(&db, gate::GateName::Segments, &agent, &mut session)
            .await
            .unwrap()
            .is_none()
    );
    let closed = gate::read(&db, gate::GateName::Segments, &mut session)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(closed.revision, 2);
    assert_eq!(closed.admission_fence, 0);
    session.abort_transaction().await.unwrap();
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn closed_gate_recovers_one_exact_admission_without_admitting_a_new_batch() {
    let (client, name, _, agent) = fixture("segment_gate_exact").await;
    let db = client.database(&name);
    let mut session = transaction(&client).await;
    gate::replace(
        &db,
        gate::GateName::Segments,
        0,
        selected(&agent),
        &mut session,
    )
    .await
    .unwrap();
    commit(&mut session).await;
    let request = operation();
    let mut session = transaction(&client).await;
    let original = admission::admit(
        &db,
        &agent,
        &request,
        admission::AdmissionKind::Activation,
        NOW,
        &mut session,
    )
    .await
    .unwrap();
    commit(&mut session).await;
    let mut session = transaction(&client).await;
    gate::replace(
        &db,
        gate::GateName::Segments,
        1,
        gate::Policy::default(),
        &mut session,
    )
    .await
    .unwrap();
    commit(&mut session).await;

    for _ in 0..3 {
        let mut session = transaction(&client).await;
        let replay = admission::admit(
            &db,
            &agent,
            &request,
            admission::AdmissionKind::Activation,
            "2026-11-06T00:00:00Z",
            &mut session,
        )
        .await
        .unwrap();
        assert_eq!(replay.admitted_at, original.admitted_at);
        assert_eq!(replay.writer_gate.revision, 1);
        commit(&mut session).await;
    }
    // A known-aborted publication may need a new lease after closure. The
    // permit retains its original base/timestamp; it does not authorize the
    // current domain transition or prove that a publication is still absent.
    let mut advanced = agent.clone();
    advanced.revision += 1;
    advanced.conversation.last_seq += 1;
    let mut session = transaction(&client).await;
    let retained = admission::admit(
        &db,
        &advanced,
        &request,
        admission::AdmissionKind::Activation,
        "2026-11-06T00:00:00Z",
        &mut session,
    )
    .await
    .unwrap();
    assert_eq!(
        retained.expected_agent_revision,
        original.expected_agent_revision
    );
    assert_eq!(retained.expected_seq, original.expected_seq);
    assert_eq!(retained.admitted_at, original.admitted_at);
    commit(&mut session).await;
    let mut regressed = retained.clone();
    regressed.expected_agent_revision = advanced.revision as i64;
    assert_eq!(
        regressed.permits(&agent, &request, admission::AdmissionKind::Activation),
        Err(SandboxStoreError::Conflict)
    );
    let mut regressed = retained;
    regressed.expected_seq = advanced.conversation.last_seq as i64;
    assert_eq!(
        regressed.permits(&agent, &request, admission::AdmissionKind::Activation),
        Err(SandboxStoreError::Conflict)
    );
    let mut another = request.clone();
    another.key = "fixture-different-publication".into();
    let mut session = transaction(&client).await;
    assert!(matches!(
        admission::admit(
            &db,
            &agent,
            &another,
            admission::AdmissionKind::Activation,
            NOW,
            &mut session
        )
        .await,
        Err(
            crate::store::agent_log_integrity::TransactionLogError::Domain(
                SandboxStoreError::Unavailable(_)
            )
        )
    ));
    session.abort_transaction().await.unwrap();
    let mut collision = request;
    collision.fingerprint = segments::publication::result_digest(&"different request").unwrap();
    let mut session = transaction(&client).await;
    assert!(matches!(
        admission::admit(
            &db,
            &agent,
            &collision,
            admission::AdmissionKind::Activation,
            NOW,
            &mut session
        )
        .await,
        Err(
            crate::store::agent_log_integrity::TransactionLogError::Domain(
                SandboxStoreError::Conflict
            )
        )
    ));
    session.abort_transaction().await.unwrap();
    assert_eq!(
        db.collection::<Document>(admission::COLLECTION)
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    let mut session = transaction(&client).await;
    assert_eq!(
        gate::read(&db, gate::GateName::Segments, &mut session)
            .await
            .unwrap()
            .unwrap()
            .admission_fence,
        1
    );
    session.abort_transaction().await.unwrap();
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn recovery_admission_requires_both_independent_cohorts_and_aborts_all_fences() {
    let (client, name, _, agent) = fixture("segment_gate_recovery").await;
    let db = client.database(&name);
    let request = operation();
    let mut session = transaction(&client).await;
    gate::replace(
        &db,
        gate::GateName::Segments,
        0,
        selected(&agent),
        &mut session,
    )
    .await
    .unwrap();
    commit(&mut session).await;
    let mut session = transaction(&client).await;
    assert!(matches!(
        admission::admit(
            &db,
            &agent,
            &request,
            admission::AdmissionKind::CapacityRecovery,
            NOW,
            &mut session
        )
        .await,
        Err(
            crate::store::agent_log_integrity::TransactionLogError::Domain(
                SandboxStoreError::Unavailable(_)
            )
        )
    ));
    session.abort_transaction().await.unwrap();
    let mut session = transaction(&client).await;
    assert_eq!(
        gate::read(&db, gate::GateName::Segments, &mut session)
            .await
            .unwrap()
            .unwrap()
            .admission_fence,
        0
    );
    gate::replace(
        &db,
        gate::GateName::CapacityRecovery,
        0,
        selected(&agent),
        &mut session,
    )
    .await
    .unwrap();
    commit(&mut session).await;
    let mut session = transaction(&client).await;
    let admitted = admission::admit(
        &db,
        &agent,
        &request,
        admission::AdmissionKind::CapacityRecovery,
        NOW,
        &mut session,
    )
    .await
    .unwrap();
    assert_eq!(admitted.recovery_gate.unwrap().revision, 1);
    assert_eq!(admitted.writer_gate.revision, 1);
    // A failure after permit creation rolls back both gate fences and the row.
    session.abort_transaction().await.unwrap();
    assert_eq!(
        db.collection::<Document>(admission::COLLECTION)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    let mut session = transaction(&client).await;
    for name in [gate::GateName::Segments, gate::GateName::CapacityRecovery] {
        assert_eq!(
            gate::read(&db, name, &mut session)
                .await
                .unwrap()
                .unwrap()
                .admission_fence,
            0
        );
    }
    session.abort_transaction().await.unwrap();
    db.drop().await.unwrap();
}
