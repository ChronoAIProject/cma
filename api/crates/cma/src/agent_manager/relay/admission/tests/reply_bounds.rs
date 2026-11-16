use super::*;
use layer2_core::sandbox::MAX_AGENT_MESSAGE_BYTES;

async fn queued(cluster: &Cluster, owner: usize, text: String) -> QueuedInputSubmission {
    let source = cluster.admit(owner, "queued-source").await.unwrap();
    let record = cluster.peers[owner]
        .writes
        .inner
        .get_command(&source.receipt.command_id)
        .await
        .unwrap()
        .unwrap();
    for peer in &cluster.peers {
        peer.writes.calls.lock().unwrap().clear();
    }
    QueuedInputSubmission {
        agent_id: cluster.id.clone(),
        target: QueuedCancellationTarget {
            response_id: source.receipt.response_id.unwrap(),
            source_command_id: source.receipt.command_id,
            expected_command_revision: record.revision,
        },
        text,
        idempotency_key: "max-edit-key".into(),
    }
}

#[tokio::test]
async fn max_size_queued_edit_returns_full_receipt_through_either_peer_and_exact_replay() {
    for text in [
        "x".repeat(MAX_AGENT_MESSAGE_BYTES),
        "\u{1}".repeat(MAX_AGENT_MESSAGE_BYTES),
    ] {
        let cluster = Cluster::new().await;
        let (owner, _) = cluster.owner().await;
        let submission = queued(&cluster, owner, text).await;
        let remote = cluster.edit(1 - owner, submission.clone()).await.unwrap();
        assert_eq!(remote.binding.text, submission.text);
        assert_eq!(
            cluster.peers[1 - owner]
                .writes
                .lookups
                .load(Ordering::SeqCst),
            0,
            "maximum reply must pass through transport without recovery masking a size failure"
        );
        let local = cluster.edit(owner, submission.clone()).await.unwrap();
        let replay = cluster.edit(1 - owner, submission.clone()).await.unwrap();
        assert_eq!(remote, local);
        assert_eq!(remote, replay);
        assert_eq!(remote.edit.receipt.agent_order, 2);
        assert_eq!(
            remote.binding.command_revision,
            submission.target.expected_command_revision + 1
        );
        let retained = cluster.peers[owner]
            .writes
            .inner
            .get_command(&submission.target.source_command_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retained.revision, remote.binding.command_revision);
        assert!(cluster.peers[1 - owner]
            .writes
            .calls
            .lock()
            .unwrap()
            .is_empty());
    }
}

#[tokio::test]
async fn lost_max_size_edit_reply_recovers_committed_receipt_without_second_mutation() {
    let cluster = Cluster::new().await;
    let (owner, _) = cluster.owner().await;
    let submission = queued(&cluster, owner, "\u{1}".repeat(MAX_AGENT_MESSAGE_BYTES)).await;
    cluster.peers[owner]
        .drop_reply
        .store(true, Ordering::SeqCst);
    let recovered = cluster.edit(1 - owner, submission.clone()).await.unwrap();
    assert_eq!(recovered.binding.text, submission.text);
    assert_eq!(
        cluster.peers[owner].writes.calls.lock().unwrap().len(),
        1,
        "lost reply recovery reads the ledger without submitting again"
    );
    let status = cluster.peers[1 - owner]
        .service
        .command_status(
            &cluster.auth.context,
            Some("test-access"),
            &recovered.edit.receipt.command_id,
        )
        .await
        .unwrap();
    assert_eq!(status, recovered.edit);
    let replay = cluster.edit(1 - owner, submission.clone()).await.unwrap();
    assert_eq!(recovered, replay);
    assert_eq!(recovered.edit.receipt.agent_order, 2);
    let source = cluster.peers[owner]
        .writes
        .inner
        .get_command(&submission.target.source_command_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(source.revision, recovered.binding.command_revision);
    assert!(cluster.peers[1 - owner]
        .writes
        .calls
        .lock()
        .unwrap()
        .is_empty());
    let mut changed = submission.clone();
    changed.text = "different input with retained key".into();
    assert!(matches!(
        cluster.peers[1 - owner]
            .service
            .lookup_queued_input(
                &cluster.auth.context,
                "test-access",
                CommandSubmissionSource::Api,
                changed
            )
            .await,
        Err(CommandAdmissionError::Ledger(
            CommandLedgerError::IdempotencyConflict
        ))
    ));
    let mut absent = submission;
    absent.idempotency_key = "absent-edit".into();
    assert!(cluster.peers[1 - owner]
        .service
        .lookup_queued_input(
            &cluster.auth.context,
            "test-access",
            CommandSubmissionSource::Api,
            absent
        )
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn valid_reply_after_owner_handoff_recovers_exact_receipt_without_new_effect() {
    let cluster = Cluster::new().await;
    let (owner, _) = cluster.owner().await;
    let submission = queued(&cluster, owner, "x".repeat(MAX_AGENT_MESSAGE_BYTES)).await;
    cluster.peers[owner]
        .release_reply
        .store(true, Ordering::SeqCst);
    let recovered = cluster.edit(1 - owner, submission.clone()).await.unwrap();
    assert_eq!(recovered.binding.text, submission.text);
    assert_eq!(
        cluster.peers[1 - owner]
            .writes
            .lookups
            .load(Ordering::SeqCst),
        1
    );
    assert_eq!(cluster.peers[owner].writes.calls.lock().unwrap().len(), 1);
    assert!(cluster.peers[1 - owner]
        .writes
        .calls
        .lock()
        .unwrap()
        .is_empty());
    let again = cluster.edit(1 - owner, submission.clone()).await.unwrap();
    assert_eq!(again, recovered);
    assert_eq!(again.edit.receipt.agent_order, 2);
    let retained = cluster.peers[1 - owner]
        .writes
        .inner
        .get_command(&submission.target.source_command_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        retained.revision,
        submission.target.expected_command_revision + 1
    );
}
