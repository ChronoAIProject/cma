use super::tests::{count, database, drain, put, sandbox, NOW};
use super::{catalog::InventorySet as Set, *};
use mongodb::{
    bson::{doc, Document},
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
    Client,
};
use std::sync::{Arc, Mutex};

fn orphan_rows(owner: &str, suffix: &str) -> Vec<(&'static str, Document)> {
    let sandbox = format!("purged-{suffix}");
    let agent = format!("orphan-agent-{suffix}");
    vec![
        (
            "resource_policies",
            doc! {"_id":&sandbox,"owner_kind":"person","owner_subject":owner},
        ),
        ("sandbox_usage", doc! {"_id":&sandbox,"owner":owner}),
        (
            "agent_creation_provenance",
            doc! {"_id":&agent,"record":{"owner":{"kind":"person","subject":owner},"sandbox_id":&sandbox,"agent_id":&agent,"workspace_id":format!("workspace-{suffix}"),"worktree_id":format!("worktree-{suffix}")}},
        ),
        (
            "agent_lifecycle_operations",
            doc! {"_id":format!("operation-{suffix}"),"owner":owner,"sandbox_id":&sandbox,"agent_id":&agent},
        ),
        (
            "sandbox_metric_bindings",
            doc! {"_id":format!("metric-{suffix}"),"owner":owner,"sandbox_id":&sandbox},
        ),
        (
            "resource_shares",
            doc! {"_id":format!("share-{suffix}"),"sandbox_id":&sandbox,"record":{"owner":owner}},
        ),
        (
            "agent_commands",
            doc! {"_id":format!("command-{suffix}"),"agent_id":&agent},
        ),
        (
            "agent_log_segments",
            doc! {"_id":format!("segment-{suffix}"),"agent_id":&agent,"conversation":"retained conversation"},
        ),
        (
            "binding_event_media_uploads",
            doc! {"_id":format!("upload-{suffix}"),"record":{"agent_id":&agent}},
        ),
    ]
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn purged_sandbox_history_is_erased_without_touching_another_owner() {
    let db = database().await;
    let owned = orphan_rows("a", "a");
    let other = orphan_rows("b", "b");
    for (collection, row) in owned.iter().chain(&other) {
        put(&db, collection, row.clone()).await;
    }
    let mut inventory = ErasureInventory::default();
    let removed = drain(&db, "a", &mut inventory).await;
    for (collection, _) in &owned {
        assert_eq!(removed.get(*collection), Some(&1), "{collection}");
        assert_eq!(count(&db, collection).await, 1, "{collection}");
    }
    for (collection, expected) in other {
        assert_eq!(
            db.collection::<Document>(collection)
                .find_one(doc! {"_id":expected.get("_id").unwrap().clone()})
                .await
                .unwrap(),
            Some(expected),
            "surviving owner's {collection} changed"
        );
    }
    assert!(inventory.sandboxes.contains(&"purged-a".into()));
    assert!(inventory.agents.contains(&"orphan-agent-a".into()));
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn historical_closure_waits_for_inventory_acknowledgement_before_effects() {
    let db = database().await;
    for (collection, row) in orphan_rows("a", "a") {
        put(&db, collection, row).await;
    }
    let subject = SubjectId::new("a");
    let inventory = ErasureInventory::default();
    let mut cursor = Cursor::default();
    for _ in 0..3 {
        let batch = erase_records(
            &db,
            &[Target {
                subject: &subject,
                inventory: &inventory,
            }],
            &mut cursor,
            NOW,
        )
        .await
        .unwrap();
        let discovered = batch
            .discovered
            .get("a")
            .expect("historical ownership must be handed off");
        assert!(discovered.sandboxes.contains(&"purged-a".into()));
        assert!(discovered.agents.contains(&"orphan-agent-a".into()));
        assert!(batch.deleted.is_empty() && batch.redacted.is_empty());
        assert!(batch.waiting && batch.has_more);
        assert_eq!(count(&db, "resource_policies").await, 1);
        assert_eq!(count(&db, "agent_creation_provenance").await, 1);
        assert_eq!(count(&db, "agent_log_segments").await, 1);
    }
    let removed = drain(&db, "a", &mut ErasureInventory::default()).await;
    assert_eq!(removed.get("agent_log_segments"), Some(&1));
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn historical_sources_independently_recover_roots_and_agent_ids() {
    let db = database().await;
    put(
        &db,
        "resource_policies",
        doc! {"_id":"policy-root","owner_kind":"person","owner_subject":"a"},
    )
    .await;
    put(&db, "sandbox_usage", doc! {"_id":"usage-root","owner":"a"}).await;
    put(
        &db,
        "sandbox_usage_events",
        doc! {"_id":"usage-event","owner":"a","sandbox_id":"event-root"},
    )
    .await;
    put(&db, "agent_creation_provenance", doc! {"_id":"provenance-agent","record":{"owner":{"kind":"person","subject":"a"},"sandbox_id":"provenance-root","workspace_id":"provenance-workspace","worktree_id":"provenance-worktree"}}).await;
    put(&db, "agent_lifecycle_operations", doc! {"_id":"operation","owner":"a","sandbox_id":"operation-root","agent_id":"operation-agent"}).await;
    put(
        &db,
        "sandbox_metric_bindings",
        doc! {"_id":"metric","owner":"a","sandbox_id":"metric-root"},
    )
    .await;
    put(
        &db,
        "resource_shares",
        doc! {"_id":"share","sandbox_id":"share-root","record":{"owner":"a"}},
    )
    .await;
    put(
        &db,
        "agent_reuse_bindings",
        doc! {"_id":"reuse","owner":"a","sandbox_id":"reuse-root","agent_id":"reuse-agent"},
    )
    .await;
    put(&db, "provider_bindings", doc! {"_id":"binding","record":{"owner":"a","sandbox_id":"binding-root","workspace_id":"binding-workspace","existing_agent":{"agent_id":"binding-agent"}}}).await;
    put(&db, "provider_binding_preparations", doc! {"_id":"preparation","owner":"a","binding_id":"old-binding","record":{"previous":{"sandbox_id":"previous-root","workspace_id":"previous-workspace"},"reserved":{"sandbox_id":"reserved-root","workspace_id":"reserved-workspace"}}}).await;
    put(&db, "native_prepared_sandboxes", doc! {"_id":"prepared-root","record":{"ownership":{"owner":"a"},"payload":{"id":"prepared-root","workspaces":[{"id":"prepared-workspace","agents":[{"id":"prepared-agent","worktree_id":"prepared-worktree"}]}]}}}).await;
    put(&db, "sandbox_usage_report_control", doc! {"_id":"diagnostic:physical","record":{"owner_subject":"a","sandbox_id":"diagnostic-root"}}).await;
    put(
        &db,
        "service_connection_handoffs",
        doc! {"_id":"handoff","owner":"a","launch_id":"historic-launch"},
    )
    .await;
    let subject = SubjectId::new("a");
    let inventory = ErasureInventory::default();
    let context = super::context::load(
        &db,
        &Target {
            subject: &subject,
            inventory: &inventory,
        },
    )
    .await
    .unwrap();
    for root in [
        "policy-root",
        "usage-root",
        "event-root",
        "provenance-root",
        "operation-root",
        "metric-root",
        "share-root",
        "reuse-root",
        "binding-root",
        "previous-root",
        "reserved-root",
        "prepared-root",
        "diagnostic-root",
    ] {
        assert!(context.ids(Set::Sandboxes).contains(&root.into()), "{root}");
    }
    for agent in [
        "provenance-agent",
        "operation-agent",
        "reuse-agent",
        "binding-agent",
        "prepared-agent",
    ] {
        assert!(context.ids(Set::Agents).contains(&agent.into()), "{agent}");
    }
    assert!(context
        .ids(Set::Workspaces)
        .contains(&"provenance-workspace".into()));
    assert!(context
        .ids(Set::Worktrees)
        .contains(&"provenance-worktree".into()));
    assert!(context.ids(Set::Bindings).contains(&"old-binding".into()));
    assert!(context
        .ids(Set::Launches)
        .contains(&"historic-launch".into()));
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn stale_launch_and_binding_inventory_cannot_erase_other_owners_children() {
    let db = database().await;
    put(&db, "trigger_launches", doc! {"_id":"launch-b","owner":"b"}).await;
    put(
        &db,
        "trigger_launch_events",
        doc! {"_id":"event-b","launch_id":"launch-b"},
    )
    .await;
    put(
        &db,
        "trigger_launch_input_protocols",
        doc! {"_id":"launch-b"},
    )
    .await;
    put(
        &db,
        "provider_bindings",
        doc! {"_id":"binding-b","record":{"owner":"b"}},
    )
    .await;
    put(
        &db,
        "binding_conversations",
        doc! {"_id":"conversation-b","binding_id":"binding-b"},
    )
    .await;
    put(
        &db,
        "trigger_launch_events",
        doc! {"_id":"event-a","launch_id":"launch-a"},
    )
    .await;
    put(
        &db,
        "trigger_launch_input_protocols",
        doc! {"_id":"launch-a"},
    )
    .await;
    put(
        &db,
        "binding_conversations",
        doc! {"_id":"conversation-a","binding_id":"binding-a"},
    )
    .await;
    let mut inventory = ErasureInventory {
        launches: vec!["launch-a".into(), "launch-b".into()],
        bindings: vec!["binding-a".into(), "binding-b".into()],
        ..Default::default()
    };
    let removed = drain(&db, "a", &mut inventory).await;
    assert_eq!(removed.get("trigger_launch_events"), Some(&1));
    assert_eq!(removed.get("trigger_launch_input_protocols"), Some(&1));
    assert_eq!(removed.get("binding_conversations"), Some(&1));
    assert_eq!(count(&db, "trigger_launches").await, 1);
    assert_eq!(count(&db, "trigger_launch_events").await, 1);
    assert_eq!(count(&db, "trigger_launch_input_protocols").await, 1);
    assert_eq!(count(&db, "binding_conversations").await, 1);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn ownership_closure_is_loaded_once_for_a_resumable_cohort() {
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let discovery = Arc::new(Mutex::new(0_usize));
    let observed = discovery.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            if event.command.get_str("find").ok() == Some("sandboxes")
                && event
                    .command
                    .get_document("filter")
                    .is_ok_and(|filter| filter.get_str("owner").ok() == Some("a"))
            {
                *observed.lock().unwrap() += 1;
            }
        }
    }));
    let db = Client::with_options(options).unwrap().database(&format!(
        "cma_erasure_cached_{}",
        uuid::Uuid::new_v4().simple()
    ));
    sandbox(&db, "a", "sandbox-a").await;
    put(
        &db,
        "agents",
        doc! {"_id":"agent-a","sandbox_id":"sandbox-a","owner":"a"},
    )
    .await;
    db.collection::<Document>("agent_log")
        .insert_many((0..1201).map(|i| doc! {"_id":format!("log-{i}"),"agent_id":"agent-a"}))
        .await
        .unwrap();
    let removed = drain(&db, "a", &mut ErasureInventory::default()).await;
    assert_eq!(removed.get("agent_log"), Some(&1201));
    assert_eq!(
        *discovery.lock().unwrap(),
        1,
        "each batch must reuse the same resource closure"
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn excessive_distinct_resource_ids_refuse_before_any_erasure() {
    let db = database().await;
    sandbox(&db, "a", "sandbox-a").await;
    db.collection::<Document>("agents")
        .insert_many(
            (0..20_001)
                .map(|i| doc! {"_id":format!("agent-{i}"),"sandbox_id":"sandbox-a","owner":"a"}),
        )
        .await
        .unwrap();
    let subject = SubjectId::new("a");
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
    let Err(Error::Blocked(block)) = result else {
        panic!("oversized closure must fail closed: {result:?}");
    };
    assert_eq!(
        block.subject_hash,
        layer2_core::account_deletion::subject_hash(&subject)
    );
    assert_eq!(
        block.reason,
        layer2_core::account_deletion::ErasureBlock::CapacityExceeded {
            class: "personal resource IDs".into(),
            limit: 20_000,
        }
    );
    assert_eq!(count(&db, "agents").await, 20_001);
    assert_eq!(count(&db, "sandboxes").await, 1);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn duplicate_usage_history_is_discovered_in_bounded_source_pages() {
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let limits = Arc::new(Mutex::new(Vec::new()));
    let observed = limits.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            if event.command.get_str("find").ok() == Some("sandbox_usage_events")
                || event.command.get_str("collection").ok() == Some("sandbox_usage_events")
            {
                observed.lock().unwrap().push(
                    event
                        .command
                        .get_i64("batchSize")
                        .or_else(|_| event.command.get_i32("batchSize").map(i64::from))
                        .unwrap_or(0),
                );
            }
        }
    }));
    let db = Client::with_options(options).unwrap().database(&format!(
        "cma_erasure_source_pages_{}",
        uuid::Uuid::new_v4().simple()
    ));
    db.collection::<Document>("sandbox_usage_events")
        .insert_many((0..1001).map(
            |i| doc! {"_id":format!("event-{i:04}"),"owner":"a","sandbox_id":"historical-sandbox"},
        ))
        .await
        .unwrap();
    let subject = SubjectId::new("a");
    let inventory = ErasureInventory::default();
    let context = super::context::load(
        &db,
        &Target {
            subject: &subject,
            inventory: &inventory,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        context.ids(Set::Sandboxes),
        vec!["historical-sandbox".to_owned()]
    );
    assert_eq!(
        *limits.lock().unwrap(),
        vec![500, 500, 500],
        "source scans must stream bounded pages and deduplicate repeated resource IDs"
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn cached_cohort_unions_new_inventory_without_trusting_foreign_sidecars() {
    let db = database().await;
    let subject = SubjectId::new("a");
    let mut inventory = ErasureInventory::default();
    let mut cursor = Cursor::default();
    for _ in 0..400 {
        let batch = erase_records(
            &db,
            &[Target {
                subject: &subject,
                inventory: &inventory,
            }],
            &mut cursor,
            NOW,
        )
        .await
        .unwrap();
        if !batch.has_more {
            break;
        }
    }
    put(
        &db,
        "agent_log",
        doc! {"_id":"late-log","agent_id":"late-orphan"},
    )
    .await;
    put(
        &db,
        "trigger_launch_events",
        doc! {"_id":"foreign-event","launch_id":"foreign-launch"},
    )
    .await;
    put(
        &db,
        "trigger_launch_input_protocols",
        doc! {"_id":"foreign-launch","owner":"b"},
    )
    .await;
    put(&db, "binding_conversations", doc! {"_id":"foreign-conversation","binding_id":"foreign-binding","record":{"owner":"b","binding_id":"foreign-binding"}}).await;
    inventory.agents.push("late-orphan".into());
    inventory.launches.push("foreign-launch".into());
    inventory.bindings.push("foreign-binding".into());
    for _ in 0..400 {
        let batch = erase_records(
            &db,
            &[Target {
                subject: &subject,
                inventory: &inventory,
            }],
            &mut cursor,
            NOW,
        )
        .await
        .unwrap();
        super::progress::acknowledge(&mut inventory, &batch, "a");
        if !batch.has_more {
            break;
        }
    }
    assert_eq!(
        count(&db, "agent_log").await,
        0,
        "new durable inventory must enter cached closure"
    );
    assert_eq!(count(&db, "trigger_launch_events").await, 1);
    assert_eq!(count(&db, "trigger_launch_input_protocols").await, 1);
    assert_eq!(count(&db, "binding_conversations").await, 1);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn sidecar_ownership_survives_missing_launch_and_binding_roots() {
    let db = database().await;
    for owner in ["a", "b"] {
        let launch = format!("launch-{owner}");
        let binding = format!("binding-{owner}");
        put(
            &db,
            "trigger_launch_events",
            doc! {"_id":format!("event-{owner}"),"launch_id":&launch},
        )
        .await;
        put(
            &db,
            "trigger_launch_input_protocols",
            doc! {"_id":&launch,"owner":owner},
        )
        .await;
        put(&db,"binding_conversations",doc! {"_id":format!("conversation-{owner}"),"binding_id":&binding,"record":{"owner":owner,"binding_id":&binding}}).await;
    }
    let mut inventory = ErasureInventory {
        launches: vec!["launch-a".into(), "launch-b".into()],
        bindings: vec!["binding-a".into(), "binding-b".into()],
        ..Default::default()
    };
    let removed = drain(&db, "a", &mut inventory).await;
    for name in [
        "trigger_launch_events",
        "trigger_launch_input_protocols",
        "binding_conversations",
    ] {
        assert_eq!(removed.get(name), Some(&1), "{name}");
        assert_eq!(count(&db, name).await, 1, "{name}");
    }
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn typed_resource_share_owner_recovers_historical_personal_lineage() {
    let db = database().await;
    for owner in ["a", "b"] {
        put(&db,"resource_shares",doc! {"_id":format!("share-{owner}"),"sandbox_id":format!("sandbox-{owner}"),"record":{"owner":{"kind":"person","subject":owner},"selector":{"agent_id":format!("agent-{owner}")}}}).await;
        put(
            &db,
            "agent_log_segments",
            doc! {"_id":format!("segment-{owner}"),"agent_id":format!("agent-{owner}")},
        )
        .await;
    }
    let mut inventory = ErasureInventory::default();
    let removed = drain(&db, "a", &mut inventory).await;
    assert_eq!(removed.get("agent_log_segments"), Some(&1));
    assert_eq!(removed.get("resource_shares"), Some(&1));
    assert_eq!(count(&db, "agent_log_segments").await, 1);
    assert_eq!(count(&db, "resource_shares").await, 1);
    assert!(inventory.agents.contains(&"agent-a".into()));
    db.drop().await.unwrap();
}
