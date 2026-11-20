//! Explain actual discovery and settlement queries against unrelated tenant data.
use super::{tests::*, *};
use mongodb::{
    bson::{doc, Bson, Document},
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
    Client,
};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct Commands(Arc<Mutex<Vec<Document>>>);
impl Commands {
    async fn database(&self) -> Database {
        let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
        let observed = self.0.clone();
        options.command_event_handler = Some(EventHandler::callback(move |event| {
            if let CommandEvent::Started(event) = event {
                if event.command_name == "find" {
                    observed.lock().unwrap().push(event.command);
                }
            }
        }));
        Client::with_options(options).unwrap().database(&format!(
            "cma_erasure_explain_{}",
            uuid::Uuid::new_v4().simple()
        ))
    }
    fn take(&self) -> Vec<Document> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

fn number(document: &Document, field: &str) -> i64 {
    document
        .get_i64(field)
        .or_else(|_| document.get_i32(field).map(i64::from))
        .unwrap()
}

fn index_scan(value: &Bson) -> bool {
    match value {
        Bson::Document(row) => {
            row.get_str("stage")
                .is_ok_and(|stage| stage == "IXSCAN" || stage == "EXPRESS_IXSCAN")
                || row.values().any(index_scan)
        }
        Bson::Array(values) => values.iter().any(index_scan),
        _ => false,
    }
}

async fn explain(db: &Database, command: &Document) -> Document {
    let mut query = Document::new();
    for field in ["find", "filter", "projection", "limit", "hint", "sort"] {
        if let Some(value) = command.get(field) {
            query.insert(field, value.clone());
        }
    }
    db.run_command(doc! {"explain":query,"verbosity":"executionStats"})
        .await
        .unwrap()
}

async fn assert_selective(db: &Database, commands: &[Document]) {
    assert!(!commands.is_empty());
    for command in commands {
        let plan = explain(db, command).await;
        let winning = plan
            .get_document("queryPlanner")
            .unwrap()
            .get("winningPlan")
            .unwrap();
        let stats = plan.get_document("executionStats").unwrap();
        assert!(index_scan(winning), "query={command:?}, plan={plan:?}");
        assert!(
            number(stats, "totalDocsExamined") <= 8,
            "query={command:?}, plan={plan:?}"
        );
        assert!(
            number(stats, "totalKeysExamined") <= 16,
            "query={command:?}, plan={plan:?}"
        );
    }
}

async fn populate(db: &Database) {
    sandbox(db, "a", "owned-root").await;
    for (name, owned) in [
        (
            "workspaces",
            doc! {"_id":"owned-workspace","owner":"a","sandbox_id":"owned-root"},
        ),
        (
            "worktrees",
            doc! {"_id":"owned-worktree","owner":"a","sandbox_id":"owned-root","workspace_id":"owned-workspace"},
        ),
        (
            "agents",
            doc! {"_id":"owned-agent","owner":"a","sandbox_id":"owned-root","workspace_id":"owned-workspace","worktree_id":"owned-worktree"},
        ),
        (
            "provider_bindings",
            doc! {"_id":"owned-binding","record":{"owner":"a","sandbox_id":"owned-root"}},
        ),
        (
            "agent_creation_provenance",
            doc! {"_id":"owned-agent","record":{"owner":{"kind":"person","subject":"a"},"sandbox_id":"owned-root","workspace_id":"owned-workspace","worktree_id":"owned-worktree"}},
        ),
        (
            "binding_conversations",
            doc! {"_id":"owned-conversation","binding_id":"owned-binding","record":{"owner":"a","binding_id":"owned-binding","sandbox_id":"owned-root","agent_id":"owned-agent","workspace_id":"owned-workspace","worktree_id":"owned-worktree"}},
        ),
        (
            "sandbox_billing_state",
            doc! {"_id":"owned-root","owner_id":"a","pending":Bson::Null,"windows":[]},
        ),
        (
            "sandbox_usage_events",
            doc! {"_id":"owned-usage","owner":"a","sandbox_id":"owned-root","billing_delivered":true},
        ),
    ] {
        put(db, name, owned).await;
    }
    // This actor-attributed organization root must not make its thousand
    // descendants part of the personal cache or subsequent refresh work.
    put(
        db,
        "agent_creation_provenance",
        doc! {"_id":"organization-agent","record":{
            "owner":{"kind":"organization","subject":"organization"},
            "created_by":{"kind":"human","subject":"a"},"sandbox_id":"organization-root",
        }},
    )
    .await;
    for name in [
        "workspaces",
        "worktrees",
        "agents",
        "agent_creation_provenance",
        "binding_conversations",
        "sandbox_billing_state",
        "sandbox_usage_events",
        "resource_policies",
    ] {
        let rows = (0..1000).map(|n| {
            let id = format!("foreign-{n:04}");
            match name {
                "agent_creation_provenance" => doc! {"_id":&id,"record":{"owner":{"kind":"person","subject":"b"},"sandbox_id":&id}},
                "binding_conversations" => doc! {"_id":&id,"binding_id":&id,"record":{"owner":"b","binding_id":&id,"sandbox_id":&id}},
                "sandbox_billing_state" => doc! {"_id":&id,"owner_id":"b","pending":{"claim":"pending"},"windows":[]},
                "sandbox_usage_events" => doc! {"_id":&id,"owner":"b","sandbox_id":&id,"billing_delivered":false,"billing_claim":"foreign-claim"},
                "resource_policies" => doc! {"_id":&id,"owner_kind":"person","owner_subject":"b"},
                _ => doc! {"_id":&id,"owner":"b","sandbox_id":"organization-root","workspace_id":&id,"worktree_id":&id},
            }
        });
        db.collection::<Document>(name)
            .insert_many(rows)
            .await
            .unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn history_and_refresh_use_selective_indexes_and_cache_personal_lineage() {
    let commands = Commands::default();
    let db = commands.database().await;
    populate(&db).await;
    commands.take();
    let subject = SubjectId::new("a");
    let inventory = ErasureInventory::default();
    let mut contexts = context::load_cohort(
        &db,
        &[Target {
            subject: &subject,
            inventory: &inventory,
        }],
    )
    .await
    .unwrap();
    let context = &mut contexts[0];
    assert_eq!(context.ids(catalog::InventorySet::Agents), ["owned-agent"]);
    assert_eq!(
        context.ids(catalog::InventorySet::Workspaces),
        ["owned-workspace"]
    );
    assert!(context.protected.contains("organization-root"));
    assert!(context.protected.len() < 10);
    let relevant = [
        "sandboxes",
        "workspaces",
        "worktrees",
        "agents",
        "agent_creation_provenance",
        "binding_conversations",
        "sandbox_billing_state",
    ];
    let loaded: Vec<_> = commands
        .take()
        .into_iter()
        .filter(|command| relevant.contains(&command.get_str("find").unwrap()))
        .collect();
    for name in [
        "workspaces",
        "agents",
        "agent_creation_provenance",
        "binding_conversations",
    ] {
        assert!(loaded
            .iter()
            .any(|command| command.get_str("find").ok() == Some(name)));
    }
    assert_selective(&db, &loaded).await;
    for _ in 0..3 {
        context.refresh(&db).await.unwrap();
    }
    let refreshed = commands.take();
    assert!(!refreshed
        .iter()
        .any(|command| command.get_str("find").ok() == Some("agent_creation_provenance")));
    for command in &refreshed {
        if ["workspaces", "worktrees", "agents"].contains(&command.get_str("find").unwrap()) {
            let filter = command.get_document("filter").unwrap();
            assert!(
                filter.contains_key("_id"),
                "refresh repeated descendant/history discovery: {command:?}"
            );
            assert!(!filter.contains_key("owner") && !filter.contains_key("sandbox_id"));
        }
    }
    assert_selective(&db, &refreshed).await;
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn billing_preflight_queries_are_bounded_and_pending_owner_still_blocks() {
    let commands = Commands::default();
    let db = commands.database().await;
    populate(&db).await;
    let subject = SubjectId::new("a");
    let inventory = ErasureInventory::default();
    let targets = [Target {
        subject: &subject,
        inventory: &inventory,
    }];
    let contexts = context::load_cohort(&db, &targets).await.unwrap();
    commands.take();
    let batch = erase_records(&db, &targets, &mut Cursor::default(), NOW)
        .await
        .unwrap();
    assert!(batch.waiting);
    guards::billing_page(&db, &contexts, &mut Cursor::default())
        .await
        .unwrap();
    let checks: Vec<_> = commands
        .take()
        .into_iter()
        .filter(|command| {
            ["sandbox_billing_state", "sandbox_usage_events"]
                .contains(&command.get_str("find").unwrap())
                && command.contains_key("limit")
        })
        .collect();
    assert_eq!(
        checks
            .iter()
            .filter(|command| command.get_str("find").ok() == Some("sandbox_billing_state"))
            .count(),
        2
    );
    assert!(checks
        .iter()
        .any(|command| command.get_str("find").ok() == Some("sandbox_usage_events")));
    for command in &checks {
        assert_eq!(number(command, "limit"), 1);
        if command.get_str("find").ok() == Some("sandbox_billing_state") {
            assert_eq!(command.get_document("projection").unwrap(), &doc! {"_id":1});
        }
    }
    assert_selective(&db, &checks).await;
    db.collection::<Document>("sandbox_billing_state")
        .update_one(
            doc! {"_id":"owned-root"},
            doc! {"$set":{"pending":{"claim":"owned-pending"}}},
        )
        .await
        .unwrap();
    let error = guards::billing_page(&db, &contexts, &mut Cursor::default())
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        Error::Blocked(AttributedBlock {
            reason: ErasureBlock::BillingSettlementPending,
            ..
        })
    ));
    let pending = commands.take();
    assert_eq!(pending.len(), 1);
    assert_selective(&db, &pending).await;
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn discovery_indexes_preserve_normal_startup_and_do_not_reuse_partial_coverage() {
    use mongodb::{options::IndexOptions, IndexModel};
    let db = database().await;
    let workspaces = db.collection::<Document>("workspaces");
    workspaces
        .create_index(
            IndexModel::builder()
                .keys(doc! {"owner":1})
                .options(
                    IndexOptions::builder()
                        .name("partial_owner_fixture".to_owned())
                        .partial_filter_expression(doc! {"owner":"a"})
                        .build(),
                )
                .build(),
        )
        .await
        .unwrap();
    indexes::ensure(&db).await.unwrap();
    let first = workspaces.list_index_names().await.unwrap();
    assert!(first.contains(&"workspaces_sandbox".to_owned()));
    assert!(
        first.contains(&"owner_1".to_owned()),
        "a partial owner index cannot cover historical owners"
    );
    indexes::ensure(&db).await.unwrap();
    assert_eq!(workspaces.list_index_names().await.unwrap(), first);
    crate::store::MongoSandboxStore::open(db.client(), db.name())
        .await
        .unwrap();
    indexes::ensure(&db).await.unwrap();
    for name in ["agents", "workspaces", "worktrees"] {
        let names = db
            .collection::<Document>(name)
            .list_index_names()
            .await
            .unwrap();
        assert!(names.contains(&format!("{name}_sandbox")));
        assert!(!names.contains(&"sandbox_id_1".to_owned()));
    }
    db.drop().await.unwrap();
}
