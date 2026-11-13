use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};

fn legacy(row: &SourceRow, config: &RuntimeConfig) -> Vec<u8> {
    let mut value: Value = serde_json::from_slice(&row.encode(config).unwrap()).unwrap();
    match row {
        SourceRow::Operation {
            identity, record, ..
        } => {
            value["identity"] = json!(identity);
            value["record"] = json!(record);
        }
        SourceRow::Authority { records, .. } => value["records"] = json!(records),
        SourceRow::Auxiliary(_) => {}
    }
    serde_json::to_vec(&value).unwrap()
}

fn digest(row: &SourceRow) -> String {
    let mut digest = Sha256::new();
    row.hash_into(&mut digest);
    format!("{:x}", digest.finalize())
}

#[tokio::test]
async fn maximum_legal_raw_columns_commit_reopen_and_preserve_exact_bytes() {
    let fixture = Fixture::new();
    let (context, mut rows, _, operation) = fixture.indexed_sources().await;
    for row in &mut rows {
        match row {
            SourceRow::Operation {
                identity, record, ..
            } => {
                identity.resize(65536, b' ');
                record.as_mut().unwrap().resize(1024 * 1024, b' ');
            }
            SourceRow::Authority { records, .. } => {
                for bytes in records.iter_mut().flatten() {
                    bytes.resize(65536, b' ');
                }
            }
            SourceRow::Auxiliary(_) => {}
        }
    }
    assert!(
        legacy(&rows[0], &fixture.config).len() > crate::ledger_storage::MAX_VALUE_BYTES as usize
    );
    let originals = rows.clone();
    let proof = proof(&context, &rows, group_key(&operation));
    let archive = fixture.indexed(true).await;
    let receipt = archive
        .prepare_indexed(proof.clone(), context.clone(), move |i| {
            Ok(rows[i as usize].clone())
        })
        .await
        .unwrap();
    archive.close().await.unwrap();
    let archive = fixture.indexed(false).await;
    let config = fixture.config.clone();
    archive
        .storage
        .call(move |db| {
            let mut query = db
                .prepare("SELECT payload FROM indexed_records ORDER BY kind,scope,source_key")
                .unwrap();
            let actual = query
                .query_map([], |r| r.get::<_, Vec<u8>>(0))
                .unwrap()
                .collect::<std::result::Result<Vec<_>, _>>()
                .unwrap();
            assert_eq!(actual.len(), originals.len());
            for (bytes, original) in actual.iter().zip(&originals) {
                let restored = SourceRow::decode(bytes, &config).unwrap();
                assert_eq!(digest(&restored), digest(original));
                assert_eq!(legacy(&restored, &config), legacy(original, &config));
            }
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(
        archive.verify_indexed(proof.clone()).await.unwrap(),
        receipt
    );
    assert_eq!(
        archive
            .prepare_indexed(proof, context, |_| panic!("reread source"))
            .await
            .unwrap(),
        receipt
    );
    assert!(archive.require_ready().await.is_err());
    assert!(archive.authorize(&[operation]).await.is_err());
    archive.close().await.unwrap();
}

#[tokio::test]
async fn earlier_numeric_archive_retries_without_rewriting_payload_or_receipt() {
    let fixture = Fixture::new();
    let (context, rows, proof, _) = fixture.indexed_sources().await;
    let old = rows
        .iter()
        .map(|row| (row.ordering(), legacy(row, &fixture.config)))
        .collect::<Vec<_>>();
    let archive = fixture.indexed(true).await;
    archive
        .prepare_indexed(proof.clone(), context.clone(), move |i| {
            Ok(rows[i as usize].clone())
        })
        .await
        .unwrap();
    let key = proof.key.clone();
    let snapshot = old.clone();
    archive.storage.call(move |db| {
        let tx = db.transaction().unwrap();
        for ((kind, scope, source_key), payload) in old {
            tx.execute("UPDATE indexed_records SET payload=?,sha256=? WHERE group_key=? AND kind=? AND scope=? AND source_key=?",
                params![payload, format!("{:x}", Sha256::digest(&payload)), key, kind, scope, source_key]).unwrap();
        }
        tx.execute("UPDATE retired_groups SET bytes=(SELECT sum(length(payload)) FROM indexed_records WHERE group_key=?)+(SELECT length(payload) FROM indexed_contexts WHERE group_key=?) WHERE group_key=?", params![key, key, key]).unwrap();
        tx.commit().unwrap();
        Ok(())
    }).await.unwrap();
    archive.close().await.unwrap();
    let archive = fixture.indexed(false).await;
    let receipt = archive.verify_indexed(proof.clone()).await.unwrap();
    assert_eq!(
        archive
            .prepare_indexed(proof, context, |_| panic!("reread source"))
            .await
            .unwrap(),
        receipt
    );
    archive.storage.call(move |db| {
        for ((kind, scope, source_key), expected) in snapshot {
            let actual: Vec<u8> = db.query_row("SELECT payload FROM indexed_records WHERE kind=? AND scope=? AND source_key=?", params![kind, scope, source_key], |r| r.get(0)).unwrap();
            assert_eq!(actual, expected);
        }
        Ok(())
    }).await.unwrap();
    archive.close().await.unwrap();
}

#[tokio::test]
async fn compact_and_numeric_codecs_preserve_null_and_refuse_bad_binary_fields() {
    let fixture = Fixture::new();
    let (_, rows, _, _) = fixture.indexed_sources().await;
    let mut pending = rows[0].clone();
    let SourceRow::Operation { status, record, .. } = &mut pending else {
        panic!()
    };
    *status = 0;
    *record = None;
    for row in rows.iter().chain([&pending]) {
        for bytes in [
            row.encode(&fixture.config).unwrap(),
            legacy(row, &fixture.config),
        ] {
            assert_eq!(
                digest(&SourceRow::decode(&bytes, &fixture.config).unwrap()),
                digest(row)
            );
        }
    }
    let operation: Value =
        serde_json::from_slice(&rows[0].encode(&fixture.config).unwrap()).unwrap();
    for value in [
        json!("!"),
        json!("AA"),
        json!([256]),
        json!([-1]),
        json!({}),
        json!(STANDARD.encode(vec![b' '; 1024 * 1024 + 1])),
    ] {
        for field in ["identity", "record"] {
            let mut bad = operation.clone();
            bad[field] = value.clone();
            assert!(
                SourceRow::decode(&serde_json::to_vec(&bad).unwrap(), &fixture.config).is_err()
            );
        }
    }
    let authority: Value =
        serde_json::from_slice(&rows[1].encode(&fixture.config).unwrap()).unwrap();
    for value in [
        json!([null, "!", null]),
        json!([null, null]),
        json!([null, null, null]),
        json!([null, "", ""]),
    ] {
        let mut bad = authority.clone();
        bad["records"] = value;
        assert!(SourceRow::decode(&serde_json::to_vec(&bad).unwrap(), &fixture.config).is_err());
    }
    let mut empty =
        serde_json::from_slice::<Value>(&pending.encode(&fixture.config).unwrap()).unwrap();
    empty["record"] = json!("");
    assert!(SourceRow::decode(&serde_json::to_vec(&empty).unwrap(), &fixture.config).is_err());
}

#[tokio::test]
async fn oversized_source_refuses_before_archival_and_leaves_no_replay_index() {
    for identity_overflow in [true, false] {
        let fixture = Fixture::new();
        let (context, mut rows, proof, operation) = fixture.indexed_sources().await;
        let SourceRow::Operation {
            identity, record, ..
        } = &mut rows[0]
        else {
            panic!()
        };
        if identity_overflow {
            identity.resize(65537, b' ');
        } else {
            record.as_mut().unwrap().resize(1024 * 1024 + 1, b' ');
        }
        let archive = fixture.indexed(true).await;
        assert!(archive
            .prepare_indexed(proof, context, move |i| Ok(rows[i as usize].clone()))
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
