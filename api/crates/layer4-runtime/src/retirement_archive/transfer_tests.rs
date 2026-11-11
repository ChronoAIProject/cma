use super::*;
use crate::retirement_archive::{probe_sources::Sources, Direction};

#[tokio::test]
async fn partial_deactivation_and_restore_remain_held_and_resume_after_restart() {
    for restore in [false, true] {
        for stop_after_move in [false, true] {
            for interrupted_row in 0..2 {
                let fixture = Fixture::new();
                let (operation, records, proof) = fixture.records("transfer");
                let sources = Arc::new(Sources::create(&fixture.path, &records).unwrap());
                let archive = fixture.open(true).await;
                archive
                    .prepare(proof.clone(), move |row| Ok(records[row as usize].clone()))
                    .await
                    .unwrap();
                if restore {
                    let selected = sources.clone();
                    archive
                        .transfer_group(proof.clone(), Direction::Deactivate, move |record| {
                            selected.transfer(record, false)
                        })
                        .await
                        .unwrap();
                }
                let selected = sources.clone();
                let mut row = 0;
                let direction = if restore {
                    Direction::RestoreHeld
                } else {
                    Direction::Deactivate
                };
                assert!(archive
                    .transfer_group(proof.clone(), direction, move |record| {
                        let stop = row == interrupted_row;
                        row += 1;
                        if stop && !stop_after_move {
                            return Err(invalid());
                        }
                        selected.transfer(record, restore)?;
                        if stop {
                            Err(invalid())
                        } else {
                            Ok(())
                        }
                    })
                    .await
                    .is_err());
                assert!(archive.require_ready().await.is_err());
                assert_eq!(
                    archive
                        .authorize(std::slice::from_ref(&operation))
                        .await
                        .unwrap_err()
                        .code,
                    "mutation_resource_retired"
                );
                archive.close().await.unwrap();
                let archive = fixture.open(false).await;
                for _ in 0..2 {
                    let selected = sources.clone();
                    let receipt = archive
                        .transfer_group(proof.clone(), direction, move |record| {
                            selected.transfer(record, restore)
                        })
                        .await
                        .unwrap();
                    assert_eq!(receipt.complete, !restore);
                }
                assert_eq!(
                    sources.counts().unwrap(),
                    if restore { (2, 0) } else { (0, 2) }
                );
                assert_eq!(archive.require_ready().await.is_ok(), !restore);
                assert_eq!(
                    archive.authorize(&[operation]).await.unwrap_err().code,
                    "mutation_resource_retired"
                );
                archive.close().await.unwrap();
            }
        }
    }
}

#[tokio::test]
async fn restore_commits_pending_before_first_source_write() {
    let fixture = Fixture::new();
    let (_, records, proof) = fixture.records("transfer");
    let sources = Arc::new(Sources::create(&fixture.path, &records).unwrap());
    let archive = fixture.open(true).await;
    archive
        .prepare(proof.clone(), move |row| Ok(records[row as usize].clone()))
        .await
        .unwrap();
    let selected = sources.clone();
    archive
        .transfer_group(proof.clone(), Direction::Deactivate, move |record| {
            selected.transfer(record, false)
        })
        .await
        .unwrap();
    archive.require_ready().await.unwrap();
    let path = fixture.path.join("ledger.sqlite3");
    let key = proof.key.clone();
    archive
        .transfer_group(proof, Direction::RestoreHeld, move |record| {
            let db = rusqlite::Connection::open_with_flags(
                &path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .unwrap();
            let complete: bool = db
                .query_row(
                    "SELECT complete FROM retired_groups WHERE group_key=?",
                    [&key],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(
                !complete,
                "restore must persist the serving hold before moving a source"
            );
            sources.transfer(record, true)
        })
        .await
        .unwrap();
    assert!(archive.require_ready().await.is_err());
    archive.close().await.unwrap();
}

#[tokio::test]
async fn invalid_proof_or_retained_payload_never_reaches_source_mutation() {
    for case in 0..4 {
        let fixture = Fixture::new();
        let (_, records, proof) = fixture.records("transfer");
        let sources = Sources::create(&fixture.path, &records).unwrap();
        let archive = fixture.open(true).await;
        archive
            .prepare(proof.clone(), move |row| Ok(records[row as usize].clone()))
            .await
            .unwrap();
        archive.close().await.unwrap();
        let mut requested = proof;
        match case {
            0 => requested.source_sha256 = "f".repeat(64),
            1 => requested.observation_sha256 = "e".repeat(64),
            2 => requested.records += 1,
            _ => {
                let db = rusqlite::Connection::open(fixture.path.join("ledger.sqlite3")).unwrap();
                db.execute(
                    "UPDATE archived_records SET sha256=? WHERE kind=2",
                    ["f".repeat(64)],
                )
                .unwrap();
                db.close().unwrap();
            }
        }
        let archive = fixture.open(false).await;
        assert!(archive
            .transfer_group(requested, Direction::Deactivate, |_| panic!(
                "unverified archived bytes cannot authorize moving sources"
            ))
            .await
            .is_err());
        assert_eq!(sources.counts().unwrap(), (2, 0));
        archive.close().await.unwrap();
    }
}

#[tokio::test]
async fn full_and_overfull_journal_and_authority_sources_reclaim_without_byte_loss() {
    let fixture = Fixture::new();
    let sources = Arc::new(Sources::create(&fixture.path, &[]).unwrap());
    let archive = fixture.open(true).await;
    let maximum = crate::mutation_journal::MAX_JOURNAL_ENTRIES + 1;
    let mut proofs = Vec::new();
    for index in 0..maximum {
        let (operation, mut records, mut proof) = fixture.records(&format!("full-{index}"));
        for fence in 1..4 {
            let record = Authority::new(&fixture.config, fence, &operation);
            records.push(SourceRecord {
                scope: 0,
                kind: 2,
                name: record
                    .inventory_name(&fixture.config)
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .into(),
                bytes: serde_json::to_vec(&record).unwrap(),
            });
        }
        records.sort_by(|a, b| (a.scope, a.kind, &a.name).cmp(&(b.scope, b.kind, &b.name)));
        let mut hash = Sha256::new();
        for record in &records {
            record.hash_into(&mut hash);
        }
        proof.source_sha256 = format!("{:x}", hash.finalize());
        proof.records = records.len() as u32;
        sources.add(&records).unwrap();
        archive
            .prepare(proof.clone(), move |row| Ok(records[row as usize].clone()))
            .await
            .unwrap();
        proofs.push(proof);
        if matches!(index + 1, 1024 | 1025) {
            assert_eq!(
                sources.counts_by_kind().unwrap(),
                [(index + 1, 0), (4 * (index + 1), 0)]
            );
        }
    }
    assert!(archive.require_ready().await.is_err());
    for proof in proofs {
        let selected = sources.clone();
        archive
            .transfer_group(proof, Direction::Deactivate, move |record| {
                selected.transfer(record, false)
            })
            .await
            .unwrap();
    }
    assert_eq!(
        sources.counts_by_kind().unwrap(),
        [(0, maximum), (0, 4 * maximum)]
    );
    archive.require_ready().await.unwrap();
    archive.close().await.unwrap();
    let archive = fixture.open(false).await;
    assert_eq!(
        archive
            .authorize(&[fixture.records("full-0").0])
            .await
            .unwrap_err()
            .code,
        "mutation_resource_retired"
    );
    let (fresh, records, proof) = fixture.records("after-full-cleanup");
    archive.authorize(&[fresh]).await.unwrap();
    sources.add(&records).unwrap();
    archive
        .prepare(proof.clone(), move |row| Ok(records[row as usize].clone()))
        .await
        .unwrap();
    let selected = sources.clone();
    archive
        .transfer_group(proof, Direction::Deactivate, move |record| {
            selected.transfer(record, false)
        })
        .await
        .unwrap();
    assert_eq!(
        sources.counts_by_kind().unwrap(),
        [(0, maximum + 1), (0, 4 * maximum + 1)]
    );
    archive.close().await.unwrap();
}

#[tokio::test]
async fn cancelled_reply_keeps_the_owned_writer_hold_until_source_work_finishes() {
    let fixture = Fixture::new();
    let (_, records, proof) = fixture.records("cancelled-transfer");
    let sources = Arc::new(Sources::create(&fixture.path, &records).unwrap());
    let archive = Arc::new(fixture.open(true).await);
    archive
        .prepare(proof.clone(), move |row| Ok(records[row as usize].clone()))
        .await
        .unwrap();
    let gate = Arc::new(tokio::sync::Mutex::new(()));
    let hold = gate.clone().lock_owned().await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let notified = entered.clone();
    let (release, wait) = std::sync::mpsc::channel();
    let retained_archive = archive.clone();
    let selected = sources.clone();
    let mut first = true;
    let task = tokio::spawn(async move {
        retained_archive
            .transfer_group(proof, Direction::Deactivate, move |record| {
                let _hold = &hold;
                if first {
                    first = false;
                    notified.notify_one();
                    wait.recv_timeout(std::time::Duration::from_secs(10))
                        .map_err(|_| invalid())?;
                }
                selected.transfer(record, false)
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
    // This query queues after the detached storage job and observes its commit.
    archive.require_ready().await.unwrap();
    assert!(
        still_held,
        "cancelling the reply must not release source writer exclusion"
    );
    assert!(gate.try_lock().is_ok());
    assert_eq!(sources.counts().unwrap(), (0, 2));
    match Arc::try_unwrap(archive) {
        Ok(archive) => archive.close().await.unwrap(),
        Err(_) => panic!("cancelled caller retained its archive handle"),
    }
}
