use super::*;
use crate::mutation_ledger::tests::Fixture;
use layer3_adapter::{AgentCloseRequest, AgentSlot};

fn close(index: usize) -> Operation {
    let agent = format!("inventory-{index:04}");
    Operation::close_agent(&AgentCloseRequest::new(
        format!("agent:{agent}:close:v1"),
        AgentSlot::new(agent),
    ))
}

async fn finish(ledger: &Ledger, index: usize) {
    let operation = close(index);
    ledger.admit(1, vec![operation.clone()]).await.unwrap();
    let entry = ledger.begin(operation.clone()).await.unwrap();
    ledger
        .finish(
            entry
                .record
                .completed(&layer3_adapter::wire::AgentCloseReceipt {
                    operation_id: operation.operation_id,
                    request_sha256: operation.request_sha256,
                    agent_id: AgentSlot::new(format!("inventory-{index:04}")),
                })
                .unwrap(),
        )
        .await
        .unwrap();
}

async fn inventory(ledger: &Ledger) -> (Vec<SourceRow>, Vec<usize>, String) {
    let mut cursor = None;
    let mut rows = Vec::new();
    let mut lengths = Vec::new();
    let mut digest = Sha256::new();
    loop {
        let page = ledger.retirement_inventory(cursor).await.unwrap();
        assert!(page.rows.len() <= ROWS);
        assert!(page.rows.iter().map(SourceRow::bytes).sum::<usize>() <= BYTES);
        lengths.push(page.rows.len());
        for row in &page.rows {
            row.hash_into(&mut digest);
        }
        rows.extend(page.rows);
        cursor = page.next;
        if cursor.is_none() {
            break;
        }
        assert!(lengths.len() < 100);
    }
    (rows, lengths, format!("{:x}", digest.finalize()))
}

#[tokio::test]
async fn retirement_inventory_pages_every_row_without_changing_ledger_or_receipts() {
    let fixture = Fixture::new();
    let ledger = fixture.open(true).await;
    for index in 0..130 {
        finish(&ledger, index).await;
    }
    let before = ledger.snapshot().await.unwrap();
    let (rows, lengths, digest) = inventory(&ledger).await;
    assert_eq!(lengths, [64, 64, 2, 64, 64, 2]);
    assert_eq!(rows.len(), 260);
    assert_eq!(ledger.snapshot().await.unwrap(), before);
    for (index, row) in rows[..130].iter().enumerate() {
        let SourceRow::Operation {
            id,
            identity,
            status,
            normal,
            record,
            ..
        } = row
        else {
            panic!("wrong source table")
        };
        let expected = close(index);
        assert_eq!(*id, expected.operation_id);
        assert_eq!(
            serde_json::from_slice::<Operation>(identity).unwrap(),
            expected
        );
        assert_eq!(*status, 2);
        assert!(*normal);
        assert_eq!(
            record.as_ref().unwrap(),
            &ledger
                .test_record(expected)
                .await
                .unwrap()
                .encode()
                .unwrap()
        );
    }
    ledger.close().await.unwrap();
    let ledger = fixture.open(false).await;
    assert_eq!(inventory(&ledger).await.2, digest);
    ledger.close().await.unwrap();
}

#[tokio::test]
async fn retirement_inventory_refuses_stale_revision_other_store_and_generation() {
    let mut fixture = Fixture::new();
    fixture.config.home_identity = Some("inventory-home".into());
    let ledger = fixture.open(true).await;
    finish(&ledger, 0).await;
    let cursor = ledger
        .retirement_inventory(None)
        .await
        .unwrap()
        .next
        .unwrap();
    let other = Fixture::new();
    let second = other.open(true).await;
    assert!(second
        .retirement_inventory(Some(cursor.clone()))
        .await
        .is_err());
    second.close().await.unwrap();
    finish(&ledger, 1).await;
    assert!(ledger
        .retirement_inventory(Some(cursor.clone()))
        .await
        .is_err());
    let current = ledger
        .retirement_inventory(None)
        .await
        .unwrap()
        .next
        .unwrap();
    ledger.close().await.unwrap();
    fixture.config.generation += 1;
    let ledger = fixture.open(false).await;
    assert!(ledger.retirement_inventory(Some(current)).await.is_err());
    let (rows, _, _) = inventory(&ledger).await;
    assert_eq!(
        rows.len(),
        4,
        "historical generation sources must remain visible"
    );
    ledger.close().await.unwrap();
}

#[tokio::test]
async fn retirement_inventory_keeps_reserved_started_failed_and_indeterminate_sources() {
    let fixture = Fixture::new();
    let ledger = fixture.open(true).await;
    for index in 0..4 {
        let operation = close(index);
        ledger
            .admit(index as u64 + 1, vec![operation.clone()])
            .await
            .unwrap();
        if index == 0 {
            continue;
        }
        let entry = ledger.begin(operation).await.unwrap();
        if index >= 2 {
            let error = crate::failure::RuntimeError::rejected(
                if index == 3 {
                    "agent_close_outcome_unknown"
                } else {
                    "inventory_failure"
                },
                "synthetic failure",
            );
            ledger
                .finish(entry.record.failed(&error, index == 3).unwrap())
                .await
                .unwrap();
        }
    }
    let before = ledger.snapshot().await.unwrap();
    let (rows, _, _) = inventory(&ledger).await;
    let status: Vec<_> = rows
        .iter()
        .filter_map(|row| match row {
            SourceRow::Operation { status, .. } => Some(*status),
            _ => None,
        })
        .collect();
    assert_eq!(status, [0, 1, 3, 4]);
    assert_eq!(ledger.snapshot().await.unwrap(), before);
    ledger.close().await.unwrap();
}

#[tokio::test]
async fn retirement_inventory_rejects_inconsistent_identity_and_unsigned_authority() {
    for authority_case in [false, true] {
        let fixture = Fixture::new();
        let ledger = fixture.open(true).await;
        finish(&ledger, 0).await;
        ledger
            .storage
            .call(move |db| {
                if authority_case {
                    db.execute(
                        "UPDATE authority SET high_water=?",
                        [u64::MAX.to_be_bytes().as_slice()],
                    )
                    .unwrap();
                } else {
                    db.execute(
                        "UPDATE operations SET identity=?",
                        [serde_json::to_vec(&close(1)).unwrap()],
                    )
                    .unwrap();
                }
                Ok(())
            })
            .await
            .unwrap();
        let first = ledger.retirement_inventory(None).await;
        if authority_case {
            assert!(ledger
                .retirement_inventory(first.unwrap().next)
                .await
                .is_err());
        } else {
            assert!(first.is_err());
        }
        ledger.close().await.unwrap();
    }
}

#[tokio::test]
async fn retirement_inventory_byte_bound_preserves_noncanonical_raw_bytes() {
    let fixture = Fixture::new();
    let ledger = fixture.open(true).await;
    for index in 0..40 {
        finish(&ledger, index).await;
    }
    ledger
        .storage
        .call(|db| {
            let tx = db.transaction().unwrap();
            for index in 0..40 {
                let id = close(index).operation_id;
                let (mut identity, mut record): (Vec<u8>, Vec<u8>) = tx
                    .query_row(
                        "SELECT identity,record FROM operations WHERE operation_id=?",
                        [&id],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .unwrap();
                // Valid, deliberately noncanonical JSON must retain its original
                // bytes. Padding also exercises the independent page-byte bound.
                identity.resize(60_000, b' ');
                record.resize(70_000, b' ');
                tx.execute(
                    "UPDATE operations SET identity=?,record=? WHERE operation_id=?",
                    params![identity, record, id],
                )
                .unwrap();
            }
            tx.commit().unwrap();
            Ok(())
        })
        .await
        .unwrap();
    let (rows, lengths, _) = inventory(&ledger).await;
    assert_eq!(rows.len(), 80);
    assert_eq!(&lengths[..3], [16, 16, 8]);
    for row in &rows[..40] {
        let SourceRow::Operation {
            identity, record, ..
        } = row
        else {
            panic!("wrong source table")
        };
        assert_eq!(identity.len(), 60_000);
        assert_eq!(record.as_ref().unwrap().len(), 70_000);
        assert_eq!(identity.last(), Some(&b' '));
        assert_eq!(record.as_ref().unwrap().last(), Some(&b' '));
    }
    ledger.close().await.unwrap();
}

#[tokio::test]
async fn retirement_inventory_queries_use_primary_key_ranges() {
    let fixture = Fixture::new();
    let ledger = fixture.open(true).await;
    ledger
        .storage
        .call(|db| {
            for sql in [OPERATIONS, AUTHORITY] {
                let plan = db
                    .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                    .unwrap()
                    .query_map(params![vec![0_u8; 8], ""], |row| row.get::<_, String>(3))
                    .unwrap()
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .unwrap()
                    .join("\n");
                assert!(
                    plan.contains("SEARCH") && plan.contains("PRIMARY KEY"),
                    "{plan}"
                );
                assert!(
                    !plan.contains("TEMP B-TREE") && !plan.contains("SCAN"),
                    "{plan}"
                );
            }
            Ok(())
        })
        .await
        .unwrap();
    ledger.close().await.unwrap();
}
