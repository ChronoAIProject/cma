use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn proven_absent_known_lineage_blocks_without_subject_projection() {
    let cases = [
        (
            "bridge_allocations",
            doc! {"sandbox_admission":{"sandbox_id":"sbx_a"}},
        ),
        (
            "bridge_allocations",
            doc! {"record":{"placement":{"target":{"sandbox_id":"sbx_a"}}}},
        ),
        (
            "channel_reuse_cleanup_tombstones",
            doc! {"sandbox_id":"sbx_a"},
        ),
        (
            "channel_reuse_cleanup_tombstones",
            doc! {"record":{"seal":{"target":{"sandbox_id":"sbx_a"}}}},
        ),
        (
            "channel_reuse_cleanup_tombstones",
            doc! {"record":{"admission":{"sandbox_id":"sbx_a"}}},
        ),
        (
            "owned_workspace_cleanup_tombstones_v1",
            doc! {"sandbox_id":"sbx_a"},
        ),
        (
            "owned_workspace_cleanup_tombstones_v1",
            doc! {"record":{"seal":{"target":{"sandbox_id":"sbx_a"}}}},
        ),
        (
            "owned_workspace_cleanup_tombstones_v1",
            doc! {"record":{"admission":{"sandbox_id":"sbx_a"}}},
        ),
    ];
    for (collection, mut row) in cases {
        let db = database().await;
        sandbox(&db, "acct:a", "sbx_a").await;
        row.insert("_id", "legacy");
        put(&db, collection, row.clone()).await;
        let subject = SubjectId::new("acct:a");
        let mut inventory = ErasureInventory::default();
        let mut cursor = Cursor::default();
        let mut blocked = false;
        for _ in 0..1000 {
            match erase_records(
                &db,
                &[Target {
                    subject: &subject,
                    inventory: &inventory,
                }],
                &mut cursor,
                NOW,
            )
            .await
            {
                Err(Error::Blocked(AttributedBlock {
                    reason: ErasureBlock::UnsupportedResidue { collection: actual },
                    ..
                })) => {
                    assert_eq!(actual, collection);
                    blocked = true;
                    break;
                }
                Ok(batch) => {
                    super::progress::acknowledge(&mut inventory, &batch, "acct:a");
                    assert!(
                        batch.has_more,
                        "missed known legacy lineage in {collection}"
                    );
                }
                other => panic!("unexpected legacy result for {collection}: {other:?}"),
            }
        }
        assert!(blocked, "legacy scan did not finish for {collection}");
        assert_eq!(
            db.collection::<Document>(collection)
                .find_one(doc! {"_id":"legacy"})
                .await
                .unwrap(),
            Some(row)
        );
        assert_eq!(count(&db, "sandboxes").await, 1);
        db.drop().await.unwrap();
    }
}

// #3296: resource execution keeps identities in its serialized `record`.
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn serialized_resource_execution_residue_blocks_erasure() {
    let record = |value: &str| doc! {"_id": "execution", "protocol": 1_i32, "record": value};
    // Another owner's evidence where the erased user acted names them only by digest.
    let actor = layer2_core::resource_access::execution::subject_execution_digests(
        &SubjectId::new("acct:a"),
        &[],
    )
    .unwrap()
    .remove(0);
    let foreign_basis = format!(r#"{{"basis":{{"owner":"other","actor":"{actor}"}}}}"#);
    for (collection, row) in [
        (
            "resource_revocation_operations_v1",
            record(r#"{"intent":{"actor":"acct:a"}}"#),
        ),
        (
            "resource_execution_records_v1",
            record(r#"{"member":{"sandbox_id":"sbx_a"}}"#),
        ),
        ("resource_execution_grants_v1", record("{unreadable")),
        (
            "resource_execution_effects_v1",
            record(r#"{"effect":["sbx_a"]}"#),
        ),
        ("resource_execution_records_v1", record(&foreign_basis)),
        // Grants name the erased Sandbox's share only by its ID.
        (
            "resource_execution_grants_v1",
            record(r#"{"key":{"origin":"d","share_id":"shr_a"}}"#),
        ),
        (
            "resource_execution_grants_v1",
            doc! {"_id": "execution", "protocol": 1_i32, "origin": "d", "share_id": "shr_a", "epoch": 1_i64},
        ),
    ] {
        let db = database().await;
        sandbox(&db, "acct:a", "sbx_a").await;
        put(
            &db,
            "resource_shares",
            doc! {"_id": "shr_a", "sandbox_id": "sbx_a"},
        )
        .await;
        put(&db, collection, row.clone()).await;
        let subject = SubjectId::new("acct:a");
        let mut inventory = ErasureInventory::default();
        let mut cursor = Cursor::default();
        let mut blocked = false;
        for _ in 0..1000 {
            match erase_records(
                &db,
                &[Target {
                    subject: &subject,
                    inventory: &inventory,
                }],
                &mut cursor,
                NOW,
            )
            .await
            {
                Err(Error::Blocked(AttributedBlock {
                    reason: ErasureBlock::UnsupportedResidue { collection: actual },
                    ..
                })) => {
                    assert_eq!(actual, collection);
                    blocked = true;
                    break;
                }
                Ok(batch) => {
                    super::progress::acknowledge(&mut inventory, &batch, "acct:a");
                    assert!(batch.has_more, "missed execution residue in {collection}");
                }
                other => panic!("unexpected result for {collection}: {other:?}"),
            }
        }
        assert!(blocked, "execution scan did not finish for {collection}");
        assert_eq!(
            db.collection::<Document>(collection)
                .find_one(doc! {"_id":"execution"})
                .await
                .unwrap(),
            Some(row)
        );
        db.drop().await.unwrap();
    }
}

const LEGACY_NAMESPACES: &[&str] = &[
    "bridge_allocation_heads",
    "bridge_allocations",
    "bridge_callback_dispositions",
    "bridge_callback_inbox",
    "bridge_callback_platform_messages",
    "bridge_callback_rates",
    "bridge_callback_receipts",
    "bridge_callback_replies",
    "bridge_channel_capabilities",
    "bridge_channel_control_aliases",
    "bridge_channel_control_intents",
    "bridge_channel_control_references",
    "bridge_credentials",
    "bridge_definition_receipts",
    "bridge_definitions",
    "bridge_invoke_audits",
    "bridge_invoke_grants",
    "bridge_invoke_receipts",
    "bridge_outbox_audits",
    "bridge_outbox_heads",
    "bridge_outbox_invocations",
    "bridge_outbox_items",
    "bridge_outbox_recoveries",
    "bridge_outbox_scans",
    "bridge_outbox_send_rates",
    "bridge_partition_command_receipts",
    "bridge_refused_control_aliases",
    "bridge_refused_control_intents",
    "bridge_source_control_audits",
    "bridge_source_control_policies",
    "bridge_source_control_receipts",
    "bridge_text_attachments",
    "channel_child_cleanup_job_admissions_v1",
    "channel_child_cleanup_jobs_v1",
    "channel_command_revival_audits",
    "channel_credential_renewal_audits",
    "channel_history_references",
    "channel_lifecycle_credential_audits",
    "channel_mint_orphan_audits",
    "channel_mint_orphans",
    "channel_provider_dispatch_journals",
    "channel_provider_finality_receipts",
    "channel_provider_teardown_receipts",
    "channel_retirement_audits",
    "channel_retirement_finalizations",
    "channel_reuse_cleanup_admission_audits",
    "channel_reuse_cleanup_admissions",
    "channel_reuse_cleanup_audits",
    "channel_reuse_cleanup_removal_audits",
    "channel_reuse_cleanup_requests",
    "channel_reuse_cleanup_tombstones",
    "owned_workspace_cleanup_audits_v1",
    "owned_workspace_cleanup_consents_v1",
    "owned_workspace_cleanup_jobs_v1",
    "owned_workspace_cleanup_tombstones_v1",
];

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn every_proven_absent_namespace_rejects_owner_rows() {
    let db = database().await;
    let mut rows = BTreeMap::new();
    // Rotate all seven candidate subject paths across the explicit legacy list.
    for (index, collection) in LEGACY_NAMESPACES.iter().enumerate() {
        let mut row = match index % 7 {
            0 => doc! {"owner":"acct:a"},
            1 => doc! {"owner_subject":"acct:a"},
            2 => doc! {"subject":"acct:a"},
            3 => doc! {"record":{"owner":"acct:a"}},
            4 => doc! {"record":{"owner":{"subject":"acct:a"}}},
            5 => doc! {"record":{"owner_subject":"acct:a"}},
            _ => doc! {"record":{"subject":"acct:a"}},
        };
        row.insert("_id", "legacy");
        put(&db, collection, row.clone()).await;
        rows.insert((*collection).to_owned(), row);
    }
    let subject = SubjectId::new("acct:a");
    let inventory = ErasureInventory::default();
    while !rows.is_empty() {
        let mut cursor = Cursor::default();
        let mut blocked = None;
        for _ in 0..1000 {
            match erase_records(
                &db,
                &[Target {
                    subject: &subject,
                    inventory: &inventory,
                }],
                &mut cursor,
                NOW,
            )
            .await
            {
                Err(Error::Blocked(AttributedBlock {
                    reason: ErasureBlock::UnsupportedResidue { collection },
                    ..
                })) => {
                    blocked = Some(collection);
                    break;
                }
                Ok(batch) => assert!(batch.has_more, "legacy owner rows were accepted"),
                other => panic!("unexpected legacy result: {other:?}"),
            }
        }
        let collection = blocked.expect("legacy scan did not finish");
        let expected = rows
            .remove(&collection)
            .expect("unexpected legacy namespace");
        let store = db.collection::<Document>(&collection);
        assert_eq!(
            store.find_one(doc! {"_id":"legacy"}).await.unwrap(),
            Some(expected)
        );
        // The executor must refuse; only this test removes the row to expose the next hit.
        store.delete_one(doc! {"_id":"legacy"}).await.unwrap();
    }
    assert!(drain(&db, "acct:a", &mut ErasureInventory::default())
        .await
        .values()
        .all(|count| *count == 0));
    db.drop().await.unwrap();
}

#[path = "catalog_billing_tests.rs"]
mod billing;
