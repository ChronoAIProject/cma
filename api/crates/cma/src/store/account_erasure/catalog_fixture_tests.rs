//! Writer-shaped ownership fixtures, independent of the executable catalog.
use super::{tests::*, *};
use mongodb::bson::{doc, Bson, Document};
use sha2::{Digest, Sha256};

fn publisher_fixture(collection: &str) -> bool {
    matches!(
        collection,
        "agent_profiles"
            | "agent_profile_revisions"
            | "workspace_profiles"
            | "workspace_profile_revisions"
            | "sandbox_profiles"
            | "sandbox_profile_revisions"
            | "trigger_definitions"
            | "trigger_definition_revisions"
    )
}

fn fixtures(subject: &str, suffix: &str) -> Vec<(&'static str, Document)> {
    let id = format!("row_{suffix}");
    let sandbox = format!("sbx_{suffix}");
    let agent = format!("agt_{suffix}");
    let launch = format!("launch_{suffix}");
    let binding = format!("binding_{suffix}");
    let report = format!("report_{suffix}");
    let mut rows = Vec::new();
    // Projected child writers in sandbox metrics, history, controls and storage.
    for collection in [
        "sandbox_metric_bindings",
        "sandbox_metric_rollups",
        "sandbox_usage_rollups",
        "runtime_backend_routes",
        "checkpoints",
        "sandbox_controls",
        "sandbox_events",
        "sandbox_sessions",
        "workspaces",
        "worktrees",
    ] {
        rows.push((collection, doc! {"_id":&id,"sandbox_id":&sandbox}));
    }
    // Agent execution writers all project the agent ID, independent of the owner.
    for collection in [
        "agent_artifacts",
        "agent_commands",
        "agent_access_tools",
        "agent_log",
        "agent_log_segments",
        "agent_log_publications",
        "agent_log_evidence",
        "agent_log_audits",
        "agent_log_admissions",
        "agent_capacity_recovery_work",
        "channel_usage_events",
        "provider_tool_relays",
        "trigger_first_message_bindings",
    ] {
        rows.push((collection, doc! {"_id":&id,"agent_id":&agent}));
    }
    for collection in [
        "agent_channel_usage",
        "agent_command_orders",
        "agent_log_heads",
        "agent_log_projections",
    ] {
        rows.push((collection, doc! {"_id":&agent}));
    }
    for collection in [
        "checkpoint_archive_gates",
        "sandbox_retained_availability",
        "warm_sandboxes",
    ] {
        rows.push((collection, doc! {"_id":&sandbox}));
    }
    for collection in [
        "agent_message_feedback",
        "agent_lifecycle_operations",
        "agent_reuse_bindings",
        "event_worker_profiles",
        "event_worker_profile_revisions",
        "provider_binding_preparations",
        "sandbox_usage",
        "sandbox_usage_events",
        "service_connection_handoffs",
        "trigger_definitions",
    ] {
        rows.push((collection, doc! {"_id":&id,"owner":subject}));
    }
    // Profile revisions intentionally carry no owner: only their parent profile ID.
    for (profiles, revisions, prefix) in [
        ("agent_profiles", "agent_profile_revisions", "ap"),
        ("workspace_profiles", "workspace_profile_revisions", "wp"),
        ("sandbox_profiles", "sandbox_profile_revisions", "sp"),
    ] {
        let profile = format!("{prefix}_{suffix}");
        rows.push((profiles, doc! {"_id":&profile,"owner":subject,"draft_version":1_i64,"last_revision":1_i64,"record":{"draft_version":1_i64}}));
        rows.push((
            revisions,
            doc! {"_id":format!("{profile}:0000000001"),"profile_id":&profile,
            "revision":1_i64,"document_schema_version":1_i64,
            "record":{"profile_id":&profile,"revision":1_i64,"content":{}}},
        ));
    }
    rows.extend([
        (
            "sandbox_usage_allocation_buckets_v1",
            doc! {"_id":format!("{sandbox}:21000"),"sandbox_id":&sandbox,"owner":subject,"day":21000_i64},
        ),
        (
            "model_budget_reservations",
            doc! {"_id":&id,"record":{"owner":subject}},
        ),
        (
            "model_budget_reservations",
            doc! {"_id":format!("owner:{subject}")},
        ),
        (
            "model_budget_reservations",
            doc! {"_id":format!("request-turn:{agent}:turn")},
        ),
        (
            "sandbox_usage_reports",
            doc! {"_id":&report,"principal":subject},
        ),
        (
            "sandbox_usage_report_rows",
            doc! {"_id":&id,"report_id":&report,"owner":subject},
        ),
        (
            "sandbox_usage_report_charts",
            doc! {"_id":&id,"report_id":&report,"owner":subject},
        ),
        (
            "visitor_launch_days",
            doc! {"_id":format!("21000:{subject}")},
        ),
        (
            "cma_document_artifacts",
            doc! {"_id":&id,"owner_subject":subject},
        ),
        (
            "native_prepared_sandboxes",
            doc! {"_id":&id,"record":{"ownership":{"owner":subject}}},
        ),
        ("activity", doc! {"_id":&id,"record":{"actor":subject}}),
        (
            "agent_suggestions",
            doc! {"_id":&id,"agent_id":&agent,"owner":subject},
        ),
        (
            "agent_creation_provenance",
            doc! {"_id":&agent,"record":{"sandbox_id":&sandbox}},
        ),
        (
            "agent_access_requests",
            doc! {"_id":&id,"record":{"requester":{"sandbox_id":&sandbox}}},
        ),
        ("agents", doc! {"_id":&agent,"sandbox_id":&sandbox}),
        (
            "binding_conversations",
            doc! {"_id":&id,"binding_id":&binding},
        ),
        (
            "binding_event_media_uploads",
            doc! {"_id":&id,"record":{"agent_id":&agent}},
        ),
        (
            "command_authorization_audits",
            doc! {"_id":&id,"record":{"agent_id":&agent}},
        ),
        (
            "operation_claims",
            doc! {"_id":{"subject":subject,"operation":"fixture","key":&id}},
        ),
        (
            "provider_tool_opens",
            doc! {"_id":&id,"record":{"scope":{"agent_id":&agent}}},
        ),
        (
            "resource_policies",
            doc! {"_id":&sandbox,"owner_kind":"person","owner_subject":subject},
        ),
        (
            "resource_share_commands",
            doc! {"_id":&id,"sandbox_id":&sandbox},
        ),
        ("resource_shares", doc! {"_id":&id,"sandbox_id":&sandbox}),
        (
            "sandbox_billing_state",
            doc! {"_id":&sandbox,"sandbox_id":&sandbox,"owner_id":subject,"pending":Bson::Null,"windows":[]},
        ),
        (
            "trigger_launch_events",
            doc! {"_id":&id,"launch_id":&launch},
        ),
        ("trigger_launch_input_protocols", doc! {"_id":&launch}),
        ("trigger_launches", doc! {"_id":&launch,"owner":subject}),
    ]);
    for collection in ["sandbox_admissions", "sandbox_colors", "users"] {
        rows.push((collection, doc! {"_id":subject}));
    }
    let hash = format!("{:x}", Sha256::digest(format!("subject:{subject}")));
    for bucket in ["default", "create", "input"] {
        rows.push((
            "request_rate_limits",
            doc! {"_id":format!("{bucket}:{hash}")},
        ));
    }
    rows
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn audited_owner_and_linked_fixtures_are_erased_without_touching_other_owner() {
    let db = database().await;
    sandbox(&db, "acct:a", "sbx_a").await;
    sandbox(&db, "acct:b", "sbx_b").await;
    let owned = fixtures("acct:a", "a")
        .into_iter()
        .filter(|(collection, _)| !publisher_fixture(collection))
        .collect::<Vec<_>>();
    let other = fixtures("acct:b", "b");
    let mut expected = BTreeMap::<String, u64>::new();
    expected.insert("sandboxes".into(), 1);
    for (collection, row) in &owned {
        put(&db, collection, row.clone()).await;
        *expected.entry((*collection).into()).or_default() += 1;
    }
    for (collection, row) in &other {
        put(&db, collection, row.clone()).await;
    }
    // Credentials are deliberately retained while their binding closure is live.
    for (subject, suffix) in [("acct:a", "a"), ("acct:b", "b")] {
        put(
            &db,
            "provider_bindings",
            doc! {"_id":format!("binding_{suffix}"),"record":{"owner":subject}},
        )
        .await;
    }
    let deleted = drain(&db, "acct:a", &mut ErasureInventory::default()).await;
    assert_eq!(deleted, expected);
    for (collection, row) in &other {
        let actual = db
            .collection::<Document>(collection)
            .find_one(doc! {"_id":row.get("_id").unwrap().clone()})
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("other owner row deleted from {collection}"));
        assert_eq!(&actual, row, "other owner row changed in {collection}");
    }
    for (collection, _) in owned {
        assert_eq!(
            count(&db, collection).await,
            expected_b_count(&other, collection),
            "{collection}"
        );
    }
    assert_eq!(count(&db, "provider_bindings").await, 2);
    assert_eq!(count(&db, "sandboxes").await, 1);
    assert!(drain(&db, "acct:a", &mut ErasureInventory::default())
        .await
        .values()
        .all(|count| *count == 0));
    db.drop().await.unwrap();
}

fn expected_b_count(rows: &[(&str, Document)], collection: &str) -> u64 {
    rows.iter().filter(|(name, _)| *name == collection).count() as u64
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn allocation_buckets_follow_owner_without_losing_recovery_authority() {
    let db = database().await;
    sandbox(&db, "acct:a", "sbx_a").await;
    for (sandbox, owner) in [
        ("sbx_a", "acct:a"),
        ("sbx_gone", "acct:a"),
        ("sbx_b", "acct:b"),
    ] {
        put(
            &db,
            "sandbox_usage_allocation_buckets_v1",
            doc! {
                "_id":format!("{sandbox}:21000"),"sandbox_id":sandbox,"owner":owner,"day":21000_i64
            },
        )
        .await;
    }
    let retained = [
        (
            "sandbox_credential_issuance",
            doc! {"_id":"sbx_a","document_schema_version":1_i64,"record":{"owner":"acct:a","state":"unknown"}},
        ),
        (
            "cma_agent_manager_members",
            doc! {"_id":"manager-a","protocol":1_i64,"draining":false},
        ),
        (
            "cma_agent_manager_owners",
            doc! {"_id":"agt_a","instance_id":"manager-a","epoch":19_i64},
        ),
    ];
    for (collection, row) in &retained {
        put(&db, collection, row.clone()).await;
    }
    let deleted = drain(&db, "acct:a", &mut ErasureInventory::default()).await;
    assert_eq!(deleted["sandbox_usage_allocation_buckets_v1"], 2);
    assert_eq!(count(&db, "sandbox_usage_allocation_buckets_v1").await, 1);
    assert!(db
        .collection::<Document>("sandbox_usage_allocation_buckets_v1")
        .find_one(doc! {"_id":"sbx_b:21000","owner":"acct:b"})
        .await
        .unwrap()
        .is_some());
    for (collection, row) in retained {
        assert_eq!(
            db.collection::<Document>(collection)
                .find_one(doc! {"_id":row.get("_id").unwrap().clone()})
                .await
                .unwrap(),
            Some(row)
        );
    }
    db.drop().await.unwrap();
}

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

// Every deferred writer-shaped fixture is tested separately, so its permanent
// refusal cannot hide another collection or consume supported fixture coverage.
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn publisher_fixture_rows_permanently_refuse_without_mutating_any_source() {
    for (collection, original) in fixtures("acct:a", "a")
        .into_iter()
        .filter(|(collection, _)| publisher_fixture(collection))
    {
        let db = database().await;
        sandbox(&db, "acct:a", "sbx_a").await;
        put(&db, "users", doc! {"_id":"acct:a","canary":"preserve"}).await;
        put(&db, collection, original.clone()).await;
        let expected = match collection {
            "agent_profile_revisions" => "agent_profiles",
            "workspace_profile_revisions" => "workspace_profiles",
            "sandbox_profile_revisions" => "sandbox_profiles",
            other => other,
        };
        if expected != collection {
            // Revision writers project the immutable parent id rather than owner.
            put(
                &db,
                expected,
                doc! {
                    "_id":original.get_str("profile_id").unwrap(),"owner":"acct:a"
                },
            )
            .await;
        }
        let owner = SubjectId::new("acct:a");
        let error = erase_records(
            &db,
            &[Target {
                subject: &owner,
                inventory: &ErasureInventory::default(),
            }],
            &mut Cursor::default(),
            NOW,
        )
        .await
        .unwrap_err();
        let Error::Blocked(AttributedBlock {
            subject_hash,
            reason,
        }) = error
        else {
            panic!("expected attributed permanent publisher refusal: {error:?}");
        };
        assert_eq!(
            subject_hash,
            layer2_core::account_deletion::subject_hash(&owner)
        );
        assert_eq!(
            reason,
            ErasureBlock::RetainedReferencePending {
                collection: expected.into()
            }
        );
        let stored = db
            .collection::<Document>(collection)
            .find_one(doc! {"_id":original.get("_id").unwrap().clone()})
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored, original, "pending source changed in {collection}");
        assert_eq!(count(&db, "sandboxes").await, 1);
        assert_eq!(count(&db, "users").await, 1);
        if expected != collection {
            let parent = db
                .collection::<Document>(expected)
                .find_one(doc! {"_id":original.get_str("profile_id").unwrap()})
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                parent,
                doc! {
                    "_id":original.get_str("profile_id").unwrap(),"owner":"acct:a"
                }
            );
        }
        db.drop().await.unwrap();
    }
}
