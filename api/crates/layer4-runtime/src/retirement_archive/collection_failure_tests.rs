use super::*;

#[tokio::test]
async fn collection_lost_replies_hold_until_acknowledged_and_reconstruct_exactly_once() {
    for stop in [
        CollectionBoundary::Selected,
        CollectionBoundary::SourceCommitted,
        CollectionBoundary::Acknowledged,
        CollectionBoundary::Closed,
    ] {
        let fixture = Fixture::new();
        let intent = fixture.intent("selected", true).await;
        let archive = fixture.collection(true).await;
        let reference = intent.retain(&archive).await;
        intent.prepare(&archive).await;
        assert!(complete_checked(
            archive,
            fixture.source(false).await,
            intent.binding.clone(),
            reference.clone(),
            intent.proof.clone(),
            (),
            move |at| {
                if std::mem::discriminant(&at) == std::mem::discriminant(&stop) {
                    Err(storage_error())
                } else {
                    Ok(())
                }
            }
        )
        .await
        .is_err());
        let source = fixture.source(false).await;
        let before = source.snapshot().await.unwrap();
        source.close().await.unwrap();
        let archive = fixture.collection(false).await;
        assert_eq!(
            archive.require_ready().await.is_err(),
            matches!(
                stop,
                CollectionBoundary::Selected | CollectionBoundary::SourceCommitted
            )
        );
        archive.close().await.unwrap();
        let receipt = fixture.finish(&intent, reference.clone()).await;
        assert_eq!(fixture.finish(&intent, reference).await, receipt);
        let source = fixture.source(false).await;
        let after = source.snapshot().await.unwrap();
        assert_eq!(
            after.revision,
            before.revision + i64::from(matches!(stop, CollectionBoundary::Selected))
        );
        assert_eq!((after.operations, after.authority), (0, 0));
        source.close().await.unwrap();
    }
}

#[tokio::test]
async fn collection_corrupt_pending_slot_or_artifact_holds_without_changing_source() {
    for sql in [
        "DELETE FROM collection_pending",
        "UPDATE collection_evidence SET pending=0",
        "UPDATE collection_artifacts SET payload=x'7b7d' WHERE kind=1",
        "DELETE FROM collection_artifacts WHERE kind=1",
        "UPDATE collection_artifacts SET ordinal=1 WHERE kind=1",
        "UPDATE collection_evidence SET request_bytes=request_bytes+1",
        "UPDATE collection_evidence SET artifacts_sha256=printf('%064d',0)",
    ] {
        let fixture = Fixture::new();
        let intent = fixture.intent("selected", true).await;
        let archive = fixture.collection(true).await;
        let reference = intent.retain(&archive).await;
        intent.prepare(&archive).await;
        archive
            .storage
            .call(move |db| {
                db.execute_batch(sql).unwrap();
                Ok(())
            })
            .await
            .unwrap();
        assert!(archive.require_ready().await.is_err());
        assert!(complete_collection_held(
            archive,
            fixture.source(false).await,
            intent.binding,
            reference,
            intent.proof,
            ()
        )
        .await
        .is_err());
        let source = fixture.source(false).await;
        assert_eq!(source.snapshot().await.unwrap().operations, 1);
        source.close().await.unwrap();
    }
}

#[tokio::test]
async fn collection_ack_failure_keeps_hold_and_retries_without_a_second_source_commit() {
    let fixture = Fixture::new();
    let intent = fixture.intent("selected", true).await;
    let archive = fixture.collection(true).await;
    let reference = intent.retain(&archive).await;
    intent.prepare(&archive).await;
    // Fault at the acknowledgement write, after the independent source commit.
    archive.storage.call(|db| {
        db.execute_batch("CREATE TEMP TRIGGER fail_ack BEFORE INSERT ON collection_acknowledgements BEGIN SELECT RAISE(ABORT,'injected'); END").unwrap();
        Ok(())
    }).await.unwrap();
    assert!(complete_collection_held(
        archive,
        fixture.source(false).await,
        intent.binding.clone(),
        reference.clone(),
        intent.proof.clone(),
        ()
    )
    .await
    .is_err());
    let source = fixture.source(false).await;
    let before = source.snapshot().await.unwrap();
    assert_eq!((before.operations, before.authority), (0, 0));
    source.close().await.unwrap();
    let archive = fixture.collection(false).await;
    assert!(archive.require_ready().await.is_err());
    assert!(archive
        .collection_completion(intent.binding.clone(), reference.clone())
        .await
        .unwrap()
        .is_none());
    archive.close().await.unwrap();
    fixture.finish(&intent, reference).await;
    let source = fixture.source(false).await;
    assert_eq!(source.snapshot().await.unwrap(), before);
    source.close().await.unwrap();
}

#[tokio::test]
async fn collection_cold_acknowledgement_refuses_missing_corrupt_or_conflicting_state() {
    for sql in [
        "DELETE FROM collection_acknowledgements",
        "UPDATE collection_acknowledgements SET receipt=x'7b7d'",
        "UPDATE collection_acknowledgements SET receipt_sha256=printf('%064d',0)",
        "UPDATE collection_artifacts SET payload=x'7b7d' WHERE kind=2",
        "UPDATE retired_groups SET complete=0",
        "DELETE FROM retired_operations",
    ] {
        let fixture = Fixture::new();
        let intent = fixture.intent("selected", true).await;
        let archive = fixture.collection(true).await;
        let reference = intent.retain(&archive).await;
        intent.prepare(&archive).await;
        archive.close().await.unwrap();
        fixture.finish(&intent, reference.clone()).await;
        let source = fixture.source(false).await;
        let before = source.snapshot().await.unwrap();
        source.close().await.unwrap();
        let archive = fixture.collection(false).await;
        archive
            .storage
            .call(move |db| {
                db.execute_batch(sql).unwrap();
                Ok(())
            })
            .await
            .unwrap();
        assert!(complete_collection_held(
            archive,
            fixture.source(false).await,
            intent.binding,
            reference,
            intent.proof,
            ()
        )
        .await
        .is_err());
        let source = fixture.source(false).await;
        assert_eq!(source.snapshot().await.unwrap(), before);
        source.close().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn collection_cancellation_owns_exclusion_through_acknowledgement_and_both_closes() {
    for stop in [
        CollectionBoundary::Selected,
        CollectionBoundary::SourceCommitted,
        CollectionBoundary::Acknowledged,
        CollectionBoundary::Closed,
    ] {
        let fixture = Fixture::new();
        let intent = fixture.intent("selected", true).await;
        let archive = fixture.collection(true).await;
        let reference = intent.retain(&archive).await;
        intent.prepare(&archive).await;
        let gate = Arc::new(tokio::sync::Mutex::new(()));
        let owned = gate.clone().lock_owned().await;
        let entered = Arc::new(tokio::sync::Notify::new());
        let signal = entered.clone();
        let (release, wait) = std::sync::mpsc::channel();
        let task = tokio::spawn(complete_checked(
            archive,
            fixture.source(false).await,
            intent.binding.clone(),
            reference.clone(),
            intent.proof.clone(),
            owned,
            move |at| {
                if std::mem::discriminant(&at) == std::mem::discriminant(&stop) {
                    signal.notify_one();
                    wait.recv_timeout(std::time::Duration::from_secs(10))
                        .map_err(|_| invalid())?;
                }
                Ok(())
            },
        ));
        tokio::time::timeout(std::time::Duration::from_secs(10), entered.notified())
            .await
            .unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(gate.try_lock().is_err());
        release.send(()).unwrap();
        let _finished = tokio::time::timeout(std::time::Duration::from_secs(10), gate.lock())
            .await
            .unwrap();
        let archive = fixture.collection(false).await;
        archive.require_ready().await.unwrap();
        let receipt = archive
            .collection_completion(intent.binding.clone(), reference.clone())
            .await
            .unwrap()
            .unwrap();
        archive.close().await.unwrap();
        assert_eq!(fixture.finish(&intent, reference).await, receipt);
    }
}
