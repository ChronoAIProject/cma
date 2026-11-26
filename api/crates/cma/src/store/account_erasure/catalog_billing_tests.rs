use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn ordinary_erasure_preserves_pending_and_acknowledged_usage_custody() {
    let db = database().await;
    let rows = [
        doc! {"_id":"pending", "owner":"acct:a", "delivery_status":"unknown",
        "record":{"source_event_id":"original-event", "payload":"frozen-evidence"}},
        doc! {"_id":"acknowledged", "owner":"acct:a", "delivery_status":"acknowledged",
        "record":{"receipt_id":"original-receipt", "payload":"frozen-evidence"}},
        doc! {"_id":"foreign", "owner":"acct:b", "record":{"payload":"foreign-evidence"}},
    ];
    for collection in crate::store::sandbox_usage_export::COLLECTIONS {
        for row in &rows {
            put(&db, collection, row.clone()).await;
        }
    }
    put(&db, "users", doc! {"_id":"acct:a"}).await;
    put(&db, "users", doc! {"_id":"acct:b"}).await;
    let deleted = drain(&db, "acct:a", &mut ErasureInventory::default()).await;
    assert_eq!(deleted.get("users"), Some(&1));
    for collection in crate::store::sandbox_usage_export::COLLECTIONS {
        assert!(!deleted.contains_key(*collection));
        assert_eq!(count(&db, collection).await, rows.len() as u64);
        for row in &rows {
            assert_eq!(
                db.collection::<Document>(collection)
                    .find_one(doc! {"_id":row.get_str("_id").unwrap()})
                    .await
                    .unwrap(),
                Some(row.clone()),
                "{collection}"
            );
        }
    }
    assert_eq!(
        db.collection::<Document>("users")
            .find_one(doc! {})
            .await
            .unwrap(),
        Some(doc! {"_id":"acct:b"})
    );
    db.drop().await.unwrap();
}

fn orphan_billing_row(owner: &str, id: &str, pending: bool) -> Document {
    use crate::sandbox::funding::{ReserveRequest, SandboxBillingState};
    let mut sandbox = crate::store::sandbox_mongo_integration_tests::sandbox(id);
    sandbox.owner = SubjectId::new(owner);
    let mut state = SandboxBillingState::new(&sandbox, 1);
    state.revision = 1;
    state.stopped = true;
    if pending {
        state.pending = Some(ReserveRequest {
            request_id: "orphan-reserve".into(),
            quote_id: "orphan-quote".into(),
            owner_id: owner.into(),
            sandbox_id: id.into(),
            runtime_generation: 1,
            operation_id: state.operation_id.clone(),
            previous_window_id: None,
            seconds: 300,
            allocation: layer2_core::sandbox_usage::Allocation {
                running: true,
                cpu_millis: 1000,
                memory_mib: 1024,
                disk_gib: 8,
            },
        });
    }
    let mut row = mongodb::bson::serialize_to_document(&state).unwrap();
    row.insert("_id", id);
    row.insert(
        "document_schema_version",
        crate::store::sandbox_mongo::DOCUMENT_SCHEMA_VERSION,
    );
    row
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn settled_orphan_billing_state_is_erased_by_owner_id() {
    let db = database().await;
    let other = orphan_billing_row("acct:b", "missing_b", false);
    put(
        &db,
        "sandbox_billing_state",
        orphan_billing_row("acct:a", "missing_a", false),
    )
    .await;
    put(&db, "sandbox_billing_state", other.clone()).await;
    put(
        &db,
        "sandbox_usage",
        doc! {"_id":"missing_a","owner":"acct:a"},
    )
    .await;
    let counts = drain(&db, "acct:a", &mut ErasureInventory::default()).await;
    assert_eq!(counts.get("sandbox_billing_state"), Some(&1));
    assert_eq!(counts.get("sandbox_usage"), Some(&1));
    assert_eq!(
        db.collection::<Document>("sandbox_billing_state")
            .find_one(doc! {"_id":"missing_a"})
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        db.collection::<Document>("sandbox_billing_state")
            .find_one(doc! {"_id":"missing_b"})
            .await
            .unwrap(),
        Some(other)
    );
    assert_eq!(count(&db, "sandboxes").await, 0);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn pending_orphan_billing_state_blocks_before_usage_erasure() {
    let db = database().await;
    let row = orphan_billing_row("acct:a", "missing_a", true);
    put(&db, "sandbox_billing_state", row.clone()).await;
    let usage = doc! {"_id":"missing_a","owner":"acct:a"};
    let event = doc! {"_id":"orphan-event","owner":"acct:a"};
    put(&db, "sandbox_usage", usage.clone()).await;
    put(&db, "sandbox_usage_events", event.clone()).await;
    let subject = SubjectId::new("acct:a");
    let inventory = ErasureInventory::default();
    let result = erase_records(
        &db,
        &[Target {
            subject: &subject,
            inventory: &inventory,
        }],
        &mut Cursor::default(),
        NOW,
    )
    .await;
    assert!(matches!(
        result,
        Err(Error::Blocked(AttributedBlock {
            reason: ErasureBlock::BillingSettlementPending,
            ..
        }))
    ));
    for (collection, expected) in [
        ("sandbox_billing_state", row),
        ("sandbox_usage", usage),
        ("sandbox_usage_events", event),
    ] {
        assert_eq!(
            db.collection::<Document>(collection)
                .find_one(doc! {"_id":expected.get("_id").unwrap().clone()})
                .await
                .unwrap(),
            Some(expected),
            "{collection}"
        );
    }
    assert_eq!(count(&db, "sandboxes").await, 0);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn settled_billing_current_receipt_does_not_block_erasure() {
    use crate::sandbox::funding::FundingWindow;
    let db = database().await;
    sandbox(&db, "acct:a", "sbx_a").await;
    let mut row = orphan_billing_row("acct:a", "sbx_a", false);
    let receipt = FundingWindow {
        request_id: "confirmed-reserve".into(),
        quote_id: "confirmed-quote".into(),
        window_id: "confirmed-window".into(),
        owner_id: "acct:a".into(),
        sandbox_id: "sbx_a".into(),
        runtime_generation: 1,
        allocation: layer2_core::sandbox_usage::Allocation {
            running: true,
            cpu_millis: 1000,
            memory_mib: 1024,
            disk_gib: 8,
        },
        covered_from: 1,
        covered_until: 301,
        receipt: serde_json::json!({"settled":true}),
    };
    row.insert(
        "current",
        mongodb::bson::serialize_to_bson(&receipt).unwrap(),
    );
    assert!(row.get_bool("stopped").unwrap());
    assert_eq!(row.get("pending"), Some(&Bson::Null));
    assert!(row.get_array("windows").unwrap().is_empty());
    put(&db, "sandbox_billing_state", row).await;
    put(&db, "sandbox_usage", doc! {"_id":"sbx_a","owner":"acct:a"}).await;
    let counts = drain(&db, "acct:a", &mut ErasureInventory::default()).await;
    assert_eq!(counts.get("sandbox_billing_state"), Some(&1));
    assert_eq!(counts.get("sandbox_usage"), Some(&1));
    assert_eq!(counts.get("sandboxes"), Some(&1));
    assert_eq!(count(&db, "sandbox_billing_state").await, 0);
    db.drop().await.unwrap();
}
