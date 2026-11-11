use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[tokio::test]
async fn indexed_visit_checks_every_source_against_the_unchanged_active_store() {
    let fixture = Fixture::new();
    let (context, rows, proof, _) = fixture.indexed_sources().await;
    let expected = rows.iter().map(SourceRow::ordering).collect::<Vec<_>>();
    let archive = fixture.indexed(true).await;
    let prepared = archive
        .prepare_indexed(proof.clone(), context.clone(), move |i| {
            Ok(rows[i as usize].clone())
        })
        .await
        .unwrap();
    let sources = Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = sources.clone();
    let expected_context = context.clone();
    assert_eq!(
        archive
            .visit_indexed_sources(proof.clone(), move |context, row| {
                assert!(context == &expected_context);
                captured.lock().unwrap().push(row);
                Ok(())
            })
            .await
            .unwrap(),
        prepared
    );
    let rows = std::mem::take(&mut *sources.lock().unwrap());
    assert_eq!(
        rows.iter().map(SourceRow::ordering).collect::<Vec<_>>(),
        expected
    );
    let ledger = Ledger::open(
        fixture.root.join("active"),
        fixture.config.clone(),
        false,
        32_768,
    )
    .await
    .unwrap();
    let before = ledger.snapshot().await.unwrap();
    ledger
        .verify_retirement_sources(context, rows)
        .await
        .unwrap();
    assert_eq!(ledger.snapshot().await.unwrap(), before);
    ledger.close().await.unwrap();
    assert!(!archive.verify_indexed(proof).await.unwrap().complete);
    assert!(archive.require_ready().await.is_err());
    archive.close().await.unwrap();
}

#[tokio::test]
async fn indexed_visit_refuses_corrupt_tail_or_replay_index_before_any_callback() {
    for replay in [false, true] {
        let fixture = Fixture::new();
        let (context, rows, proof, _) = fixture.indexed_sources().await;
        let archive = fixture.indexed(true).await;
        archive
            .prepare_indexed(
                proof.clone(),
                context,
                move |i| Ok(rows[i as usize].clone()),
            )
            .await
            .unwrap();
        archive.storage.call(move |db| {
            db.execute_batch(if replay {
                "DELETE FROM retired_operations"
            } else {
                "UPDATE indexed_records SET sha256=printf('%064d',0) WHERE kind=(SELECT max(kind) FROM indexed_records)"
            }).unwrap();
            Ok(())
        }).await.unwrap();
        let called = Arc::new(AtomicUsize::new(0));
        let observed = called.clone();
        assert!(archive
            .visit_indexed_sources(proof, move |_, _| {
                observed.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .await
            .is_err());
        assert_eq!(called.load(Ordering::SeqCst), 0);
        assert!(archive.require_ready().await.is_err());
        archive.close().await.unwrap();
    }
}

#[tokio::test]
async fn indexed_visit_failure_stops_without_completing_or_changing_the_archive() {
    let fixture = Fixture::new();
    let (context, rows, proof, _) = fixture.indexed_sources().await;
    let archive = fixture.indexed(true).await;
    let before = archive
        .prepare_indexed(
            proof.clone(),
            context,
            move |i| Ok(rows[i as usize].clone()),
        )
        .await
        .unwrap();
    let called = Arc::new(AtomicUsize::new(0));
    let observed = called.clone();
    assert!(archive
        .visit_indexed_sources(proof.clone(), move |_, _| {
            observed.fetch_add(1, Ordering::SeqCst);
            Err(invalid())
        })
        .await
        .is_err());
    assert_eq!(called.load(Ordering::SeqCst), 1);
    assert_eq!(archive.verify_indexed(proof).await.unwrap(), before);
    archive.close().await.unwrap();
}

#[tokio::test]
async fn indexed_visit_cancellation_keeps_the_owned_guard_until_worker_completion() {
    let fixture = Fixture::new();
    let (context, rows, proof, _) = fixture.indexed_sources().await;
    let archive = Arc::new(fixture.indexed(true).await);
    let before = archive
        .prepare_indexed(
            proof.clone(),
            context,
            move |i| Ok(rows[i as usize].clone()),
        )
        .await
        .unwrap();
    let gate = Arc::new(tokio::sync::Mutex::new(()));
    let hold = gate.clone().lock_owned().await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let notification = entered.clone();
    let (release, wait) = std::sync::mpsc::channel();
    let handle = archive.clone();
    let selected = proof.clone();
    let task = tokio::spawn(async move {
        let mut first = true;
        handle
            .visit_indexed_sources(selected, move |_, _| {
                let _hold = &hold;
                if first {
                    first = false;
                    notification.notify_one();
                    wait.recv_timeout(std::time::Duration::from_secs(10))
                        .map_err(|_| invalid())?;
                }
                Ok(())
            })
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), entered.notified())
        .await
        .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let still_held = gate.try_lock().is_err();
    release.send(()).unwrap();
    assert_eq!(archive.verify_indexed(proof).await.unwrap(), before);
    assert!(still_held);
    assert!(gate.try_lock().is_ok());
    match Arc::try_unwrap(archive) {
        Ok(archive) => archive.close().await.unwrap(),
        Err(_) => panic!("visitor retained archive owner"),
    }
}
