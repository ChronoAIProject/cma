use super::*;
#[path = "indexed_encoding_tests.rs"]
mod encoding_tests;
#[path = "indexed_visit_tests.rs"]
mod visit_tests;
use crate::mutation_ledger::{
    retirement_inventory::{Context, SourceRow},
    Ledger,
};

impl Fixture {
    async fn indexed(&self, create: bool) -> Archive {
        Archive::open_version(
            self.path.clone(),
            self.config.clone(),
            self.id.clone(),
            create,
            32_768,
            Version::Indexed,
        )
        .await
        .unwrap()
    }

    async fn indexed_sources(&self) -> (Context, Vec<SourceRow>, GroupProof, Operation) {
        let ledger = Ledger::open(self.root.join("active"), self.config.clone(), true, 32_768)
            .await
            .unwrap();
        let (operation, _, _) = self.records("indexed");
        ledger
            .admit(u64::MAX, vec![operation.clone()])
            .await
            .unwrap();
        let entry = ledger.begin(operation.clone()).await.unwrap();
        ledger
            .finish(
                entry
                    .record
                    .completed(&layer3_adapter::wire::AgentHomeRemoveReceipt {
                        operation_id: operation.operation_id.clone(),
                        request_sha256: operation.request_sha256.clone(),
                        agent_id: AgentSlot::new("indexed"),
                    })
                    .unwrap(),
            )
            .await
            .unwrap();
        let mut context = None;
        let mut cursor = None;
        let mut rows = Vec::new();
        loop {
            let page = ledger.retirement_inventory(cursor).await.unwrap();
            context.get_or_insert(page.context);
            rows.extend(page.rows);
            cursor = page.next;
            if cursor.is_none() {
                break;
            }
        }
        ledger.close().await.unwrap();
        let context = context.unwrap();
        let proof = proof(&context, &rows, group_key(&operation));
        (context, rows, proof, operation)
    }
}

fn proof(context: &Context, rows: &[SourceRow], key: String) -> GroupProof {
    let mut source = Sha256::new();
    context.hash_into(&mut source);
    for row in rows {
        row.hash_into(&mut source);
    }
    GroupProof {
        key,
        source_sha256: format!("{:x}", source.finalize()),
        observation_sha256: "b".repeat(64),
        records: rows.len() as u32,
    }
}

#[tokio::test]
async fn indexed_archive_preserves_raw_columns_and_retries_after_reopen_without_reading_source() {
    let fixture = Fixture::new();
    let (context, mut rows, _, operation) = fixture.indexed_sources().await;
    // Preserve exact noncanonical source bytes instead of re-encoding a decoder.
    for row in &mut rows {
        match row {
            SourceRow::Operation {
                identity, record, ..
            } => {
                identity.extend_from_slice(b" \n");
                record.as_mut().unwrap().extend_from_slice(b"\n");
            }
            SourceRow::Authority { records, .. } => {
                for record in records.iter_mut().flatten() {
                    record.extend_from_slice(b"\n ");
                }
            }
            _ => {}
        }
    }
    let proof = proof(&context, &rows, group_key(&operation));
    let originals = rows
        .iter()
        .map(|row| row.encode(&fixture.config).unwrap())
        .collect::<Vec<_>>();
    let original_context = context.encode(&fixture.config).unwrap();
    let archive = fixture.indexed(true).await;
    let receipt = archive
        .prepare_indexed(proof.clone(), context.clone(), move |i| {
            Ok(rows[i as usize].clone())
        })
        .await
        .unwrap();
    assert!(!receipt.complete);
    assert!(archive.require_ready().await.is_err());
    assert_eq!(
        archive
            .authorize(std::slice::from_ref(&operation))
            .await
            .unwrap_err()
            .code,
        "mutation_resource_retired"
    );
    archive
        .storage
        .call(move |db| {
            let payload: Vec<u8> = db
                .query_row("SELECT payload FROM indexed_contexts", [], |r| r.get(0))
                .unwrap();
            assert_eq!(payload, original_context);
            let actual = db
                .prepare("SELECT payload FROM indexed_records ORDER BY kind,scope,source_key")
                .unwrap()
                .query_map([], |r| r.get::<_, Vec<u8>>(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            assert_eq!(actual, originals);
            Ok(())
        })
        .await
        .unwrap();
    archive.close().await.unwrap();
    let archive = fixture.indexed(false).await;
    assert_eq!(
        archive.verify_indexed(proof.clone()).await.unwrap(),
        receipt
    );
    assert_eq!(
        archive
            .prepare_indexed(proof.clone(), context, |_| panic!("retry reread sources"))
            .await
            .unwrap(),
        receipt
    );
    let later = Operation::close_agent(&layer3_adapter::AgentCloseRequest::new(
        "agent:indexed:close:v1",
        AgentSlot::new("indexed"),
    ));
    assert_eq!(
        archive.authorize(&[later]).await.unwrap_err().code,
        "mutation_resource_retired"
    );
    assert!(archive
        .transfer_group(proof, Direction::Deactivate, |_| panic!(
            "V1 transfer used for V2 rows"
        ))
        .await
        .is_err());
    archive.close().await.unwrap();
}

#[tokio::test]
async fn indexed_prepare_rolls_back_every_replay_index_on_bad_selection_or_copy_failure() {
    for fault in 0..6 {
        let fixture = Fixture::new();
        let (context, mut rows, mut proof, operation) = fixture.indexed_sources().await;
        match fault {
            0 => proof.source_sha256 = "0".repeat(64),
            1 => proof.key = "0".repeat(64),
            2 => rows.swap(0, 1),
            3 => {
                let SourceRow::Operation { status, .. } = &mut rows[0] else {
                    panic!()
                };
                *status = 1;
            }
            4 => proof.records += 1,
            _ => {}
        }
        let archive = fixture.indexed(true).await;
        assert!(archive
            .prepare_indexed(proof, context, move |i| {
                if (fault == 5 && i == 1) || i as usize >= rows.len() {
                    return Err(invalid());
                }
                Ok(rows[i as usize].clone())
            })
            .await
            .is_err());
        archive.require_ready().await.unwrap();
        archive.authorize(&[operation]).await.unwrap();
        archive
            .storage
            .call(|db| {
                for table in [
                    "retired_groups",
                    "retired_operations",
                    "indexed_records",
                    "indexed_contexts",
                ] {
                    let count: i64 = db
                        .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                        .unwrap();
                    assert_eq!(count, 0);
                }
                Ok(())
            })
            .await
            .unwrap();
        archive.close().await.unwrap();
    }
}

#[tokio::test]
async fn indexed_verification_refuses_context_row_key_payload_and_replay_index_damage() {
    for fault in 0..5 {
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
        archive
            .storage
            .call(move |db| {
                db.execute_batch(match fault {
                    0 => "UPDATE indexed_contexts SET sha256=printf('%064d',0)",
                    1 => "UPDATE indexed_records SET source_key='different' WHERE kind=3",
                    2 => "UPDATE indexed_records SET payload=x'7b7d' WHERE kind=3",
                    3 => "DELETE FROM retired_operations",
                    _ => "DELETE FROM indexed_records WHERE kind=5",
                })
                .unwrap();
                Ok(())
            })
            .await
            .unwrap();
        assert!(archive.verify_indexed(proof).await.is_err());
        assert!(archive.require_ready().await.is_err());
        archive.close().await.unwrap();
    }
}

#[tokio::test]
async fn indexed_retry_refuses_a_different_context_and_cross_version_interpretation() {
    let fixture = Fixture::new();
    let (context, rows, proof, _) = fixture.indexed_sources().await;
    let archive = fixture.indexed(true).await;
    archive
        .prepare_indexed(proof.clone(), context.clone(), move |i| {
            Ok(rows[i as usize].clone())
        })
        .await
        .unwrap();
    let mut changed = serde_json::to_value(&context).unwrap();
    changed["accounting"][0] = serde_json::json!(12345);
    let changed = Context::decode(&serde_json::to_vec(&changed).unwrap(), &fixture.config).unwrap();
    assert!(archive
        .prepare_indexed(proof, changed, |_| panic!("retry must not read sources"))
        .await
        .is_err());
    archive.close().await.unwrap();
    assert!(Archive::open(
        fixture.path.clone(),
        fixture.config.clone(),
        fixture.id.clone(),
        false,
        32_768
    )
    .await
    .is_err());
    fixture.indexed(false).await.close().await.unwrap();
    let legacy = Fixture::new();
    legacy.open(true).await.close().await.unwrap();
    assert!(Archive::open_version(
        legacy.path.clone(),
        legacy.config.clone(),
        legacy.id.clone(),
        false,
        32_768,
        Version::Indexed
    )
    .await
    .is_err());
    legacy.open(false).await.close().await.unwrap();
}

#[tokio::test]
async fn indexed_payload_codec_refuses_unknown_shapes_invalid_columns_and_foreign_context() {
    let fixture = Fixture::new();
    let (context, rows, _, _) = fixture.indexed_sources().await;
    for row in &rows {
        let bytes = row.encode(&fixture.config).unwrap();
        assert_eq!(
            SourceRow::decode(&bytes, &fixture.config)
                .unwrap()
                .encode(&fixture.config)
                .unwrap(),
            bytes
        );
        let mut value = serde_json::from_slice::<serde_json::Value>(&bytes).unwrap();
        value["unexpected"] = true.into();
        assert!(SourceRow::decode(&serde_json::to_vec(&value).unwrap(), &fixture.config).is_err());
    }
    let mut bad = rows[0].clone();
    let SourceRow::Operation { scope, .. } = &mut bad else {
        panic!()
    };
    *scope = vec![0; 7];
    assert!(bad.encode(&fixture.config).is_err());
    let mut bad = rows[1].clone();
    let SourceRow::Authority { high, .. } = &mut bad else {
        panic!()
    };
    *high = 1_u64.to_be_bytes().to_vec();
    assert!(bad.encode(&fixture.config).is_err());
    let mut config = fixture.config.clone();
    config.sandbox_id = "foreign".into();
    assert!(Context::decode(&context.encode(&fixture.config).unwrap(), &config).is_err());
    let mut future = fixture.config.clone();
    future.generation += 1;
    assert!(Context::decode(&context.encode(&fixture.config).unwrap(), &future).is_ok());
    assert!(SourceRow::decode(&vec![b' '; 8 * 1024 * 1024 + 1], &fixture.config).is_err());
}

#[tokio::test]
async fn indexed_cancelled_reply_keeps_selection_hold_and_commits_an_idempotent_pending_group() {
    let fixture = Fixture::new();
    let (context, rows, proof, operation) = fixture.indexed_sources().await;
    let archive = Arc::new(fixture.indexed(true).await);
    let gate = Arc::new(tokio::sync::Mutex::new(()));
    let hold = gate.clone().lock_owned().await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let notification = entered.clone();
    let (release, wait) = std::sync::mpsc::channel();
    let handle = archive.clone();
    let selected = proof.clone();
    let original_context = context.clone();
    let task = tokio::spawn(async move {
        handle
            .prepare_indexed(selected, context, move |i| {
                let _hold = &hold;
                if i == 1 {
                    notification.notify_one();
                    wait.recv_timeout(std::time::Duration::from_secs(10))
                        .map_err(|_| invalid())?;
                }
                Ok(rows[i as usize].clone())
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
    let receipt = archive.verify_indexed(proof.clone()).await.unwrap();
    assert!(still_held);
    assert!(gate.try_lock().is_ok());
    assert!(!receipt.complete);
    assert!(archive.require_ready().await.is_err());
    assert_eq!(
        archive.authorize(&[operation]).await.unwrap_err().code,
        "mutation_resource_retired"
    );
    assert_eq!(
        archive
            .prepare_indexed(proof, original_context, |_| panic!("retry reread source"))
            .await
            .unwrap(),
        receipt
    );
    match Arc::try_unwrap(archive) {
        Ok(archive) => archive.close().await.unwrap(),
        Err(_) => panic!("cancelled reply retained archive owner"),
    }
}
