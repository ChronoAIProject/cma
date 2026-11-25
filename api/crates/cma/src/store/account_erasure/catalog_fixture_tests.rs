//! Writer-shaped ownership fixtures, independent of the executable catalog.
use super::{tests::*, *};
use mongodb::bson::{doc, Bson, Document};
use sha2::{Digest, Sha256};

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
            "sandbox_creation_proofs",
            doc! {"_id": &id,"owner":subject},
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
    let owned = fixtures("acct:a", "a");
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

#[path = "catalog_residue_tests.rs"]
mod residue;
