//! Real persistence regressions for the second erasure review.
use super::{tests::*, *};
use layer2_core::profiles::AgentProfileStore;
use mongodb::bson::{doc, Document};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn retained_profile_is_private_retired_and_contains_no_authoring_data() {
    let seed =
        crate::store::trigger_definitions::integration_tests::seeded("erasure_retired_profile")
            .await;
    let db = seed.client.database(&seed.database);
    let id = seed.target.agent_profile.id.as_str();
    let revision = seed
        .profiles
        .agent_profile_revision(&seed.target.agent_profile.id, 1)
        .await
        .unwrap()
        .unwrap();
    db.collection::<Document>("agent_profiles").update_one(doc! {"_id":id}, doc! {"$set":{
        "name":"Private authoring name","record.name":"Private authoring name",
        "description":"Private authoring description","record.description":"Private authoring description",
        "record.draft.system_prompt":"Unpublished private prompt"
    }}).await.unwrap();
    let mut unused = db
        .collection::<Document>("agent_profile_revisions")
        .find_one(doc! {"profile_id":id})
        .await
        .unwrap()
        .unwrap();
    unused.insert("_id", format!("{id}:0000000002"));
    unused.insert("revision", 2_i64);
    unused
        .get_document_mut("record")
        .unwrap()
        .insert("revision", 2_i64);
    db.collection::<Document>("agent_profile_revisions")
        .insert_one(unused)
        .await
        .unwrap();
    put(&db,"agents",doc! {"_id":"foreign-reference","owner":"visitor","record":{"profile":{"id":id,"revision":1_i64}}}).await;
    drain(&db, seed.owner.as_str(), &mut ErasureInventory::default()).await;
    let retained = db
        .collection::<Document>("agent_profiles")
        .find_one(doc! {"_id":id})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.get_str("visibility").unwrap(), "private");
    assert!(retained.get_str("retired_at").is_ok());
    assert_eq!(retained.get_str("name").unwrap(), "");
    assert_eq!(retained.get_str("description").unwrap(), "");
    assert!(!retained
        .get_document("record")
        .unwrap()
        .contains_key("draft"));
    let parsed = seed
        .profiles
        .agent_profile(&seed.target.agent_profile.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(parsed.owner.as_str(), layer2_core::DELETED_ACCOUNT_SUBJECT);
    assert_eq!(
        seed.profiles
            .agent_profile_revision(&seed.target.agent_profile.id, 1)
            .await
            .unwrap(),
        Some(revision)
    );
    assert!(seed
        .profiles
        .agent_profile_revision(&seed.target.agent_profile.id, 2)
        .await
        .unwrap()
        .is_none());
    assert!(seed
        .profiles
        .publish_agent_profile(
            &seed.target.agent_profile.id,
            parsed.draft_version,
            NOW.into()
        )
        .await
        .is_err());
    db.drop().await.unwrap();
}

#[test]
fn capacity_is_attributed_per_person_and_does_not_count_organization_children() {
    let mut contexts = vec![];
    for subject in ["a", "b"] {
        let mut context = context::Context::default();
        context.subject = subject.into();
        context.sets.insert(
            catalog::InventorySet::Agents,
            (0..11000).map(|n| format!("{subject}-{n}")).collect(),
        );
        context
            .protected
            .extend((0..21000).map(|n| format!("organization-child-{n}")));
        contexts.push(context);
    }
    assert!(progress::capacity(&contexts).is_ok());
    contexts[1]
        .sets
        .get_mut(&catalog::InventorySet::Agents)
        .unwrap()
        .extend((11000..20001).map(|n| format!("b-{n}")));
    let error = progress::capacity(&contexts).unwrap_err();
    assert!(format!("{error:?}").contains(&layer2_core::document_artifacts::digest(b"b")));
    assert!(format!("{error:?}").contains("CapacityExceeded"));
}

#[test]
fn serialized_cursor_capacity_refuses_before_sixteen_megabytes() {
    let mut context = context::Context::default();
    context.subject = "a".into();
    context.sets.insert(
        catalog::InventorySet::Agents,
        (0..1000)
            .map(|n| format!("{n}-{}", "x".repeat(2000)))
            .collect(),
    );
    let error = progress::capacity(&[context]).unwrap_err();
    assert!(format!("{error:?}").contains("CapacityExceeded"));
    assert!(format!("{error:?}").contains(&layer2_core::document_artifacts::digest(b"a")));
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn damaged_owned_root_is_typed_and_identifies_its_subject() {
    let db = database().await;
    put(&db, "sandboxes", doc! {"_id":true,"owner":"a"}).await;
    let subject = SubjectId::new("a");
    let error = erase_records(
        &db,
        &[Target {
            subject: &subject,
            inventory: &ErasureInventory::default(),
        }],
        &mut Cursor::default(),
        NOW,
    )
    .await
    .unwrap_err();
    assert!(format!("{error:?}").contains("CorruptRecord"));
    assert!(format!("{error:?}").contains(&layer2_core::document_artifacts::digest(b"a")));
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn erasure_installs_majority_fences_before_any_reference_discovery() {
    let seed =
        crate::store::trigger_definitions::integration_tests::seeded("erasure_drain_barrier").await;
    let db = seed.client.database(&seed.database);
    let subject = seed.owner.clone();
    let batch = erase_records_with_options(
        &db,
        &[Target {
            subject: &subject,
            inventory: &ErasureInventory::default(),
        }],
        &mut Cursor::default(),
        NOW,
        Options {
            allocation_rollups_enabled: Some(false),
            agent_shared_observation: Some(false),
            retain_legal_consents: false,
            drain_qualification: Some(TransactionDrainQualification::fixture()),
            billing_enabled: false,
            transaction_drain_minimum_secs: 120,
        },
    )
    .await
    .unwrap();
    assert!(batch.waiting && batch.has_more);
    assert!(batch.deleted.is_empty() && batch.redacted.is_empty());
    for (name, id) in [
        ("sandbox_profiles", seed.target.sandbox_profile.id.as_str()),
        (
            "workspace_profiles",
            seed.target.workspace_profile.id.as_str(),
        ),
        ("agent_profiles", seed.target.agent_profile.id.as_str()),
    ] {
        let row = db
            .collection::<Document>(name)
            .find_one(doc! {"_id":id})
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.get_bool("account_erasure_fenced").ok(), Some(true));
        assert!(row.get_datetime("account_erasure_fenced_at").is_ok());
    }
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn profile_reference_sources_are_scanned_once_for_all_revision_rows() {
    use mongodb::{
        event::{command::CommandEvent, EventHandler},
        options::ClientOptions,
        Client,
    };
    use std::sync::{Arc, Mutex};
    let seed =
        crate::store::trigger_definitions::integration_tests::seeded("erasure_reference_scan_once")
            .await;
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let reads = Arc::new(Mutex::new(0usize));
    let observed = reads.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            if event.command.get_str("find").ok() == Some("agents")
                && event
                    .command
                    .get_document("projection")
                    .ok()
                    .is_some_and(|projection| projection.contains_key("record.profile"))
            {
                *observed.lock().unwrap() += 1;
            }
        }
    }));
    let db = Client::with_options(options)
        .unwrap()
        .database(&seed.database);
    let template = db
        .collection::<Document>("agent_profile_revisions")
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap();
    let id = seed.target.agent_profile.id.as_str();
    for n in 2..12 {
        let mut row = template.clone();
        row.insert("_id", format!("{id}:{n:010}"));
        row.insert("revision", n as i64);
        row.get_document_mut("record")
            .unwrap()
            .insert("revision", n as i64);
        put(&db, "agent_profile_revisions", row).await;
    }
    drain(&db, seed.owner.as_str(), &mut ErasureInventory::default()).await;
    assert_eq!(
        *reads.lock().unwrap(),
        1,
        "each source must serve the entire cohort once, observed {} reads",
        *reads.lock().unwrap()
    );
    assert_eq!(count(&db, "agent_profile_revisions").await, 0);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn ownership_discovery_explain_uses_the_owner_index_without_examining_other_tenants() {
    use mongodb::{
        event::{command::CommandEvent, EventHandler},
        options::ClientOptions,
        Client, IndexModel,
    };
    use std::sync::{Arc, Mutex};
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let commands = Arc::new(Mutex::new(Vec::<Document>::new()));
    let observed = commands.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            if event.command.get_str("find").ok() == Some("sandboxes")
                && event
                    .command
                    .get_document("filter")
                    .is_ok_and(|f| f.get_str("owner").ok() == Some("a"))
            {
                observed.lock().unwrap().push(event.command.clone());
            }
        }
    }));
    let db = Client::with_options(options).unwrap().database(&format!(
        "cma_erasure_query_{}",
        uuid::Uuid::new_v4().simple()
    ));
    db.collection::<Document>("sandboxes")
        .create_index(IndexModel::builder().keys(doc! {"owner":1,"_id":1}).build())
        .await
        .unwrap();
    sandbox(&db, "a", "zz-owned").await;
    db.collection::<Document>("sandboxes")
        .insert_many((0..1000).map(|n| doc! {"_id":format!("other-{n:04}"),"owner":"b"}))
        .await
        .unwrap();
    let subject = SubjectId::new("a");
    let context = context::load(
        &db,
        &Target {
            subject: &subject,
            inventory: &ErasureInventory::default(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        context.ids(catalog::InventorySet::Sandboxes),
        vec!["zz-owned"]
    );
    let command = commands.lock().unwrap().first().cloned().unwrap();
    let mut query = Document::new();
    for field in ["find", "filter", "projection", "sort", "hint", "limit"] {
        if let Some(value) = command.get(field) {
            query.insert(field, value.clone());
        }
    }
    let explain = db
        .run_command(doc! {"explain":query,"verbosity":"executionStats"})
        .await
        .unwrap();
    let stats = explain.get_document("executionStats").unwrap();
    assert!(
        stats
            .get_i32("totalDocsExamined")
            .map(i64::from)
            .or_else(|_| stats.get_i64("totalDocsExamined"))
            .unwrap()
            <= 2,
        "{explain:?}"
    );
    assert!(format!("{explain:?}").contains("IXSCAN"));
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn indexed_pages_protect_every_lineage_and_reach_later_owned_rows() {
    let db = database().await;
    indexes::ensure(&db).await.unwrap();
    let entry = catalog::ERASURE
        .iter()
        .find(|entry| entry.collection == "agent_commands")
        .unwrap();
    for field in [
        "workspace_id",
        "worktree_id",
        "launch_id",
        "binding_id",
        "record.binding_id",
    ] {
        let mut context = context::Context::default();
        context.subject = "a".into();
        context.sets.insert(
            catalog::InventorySet::Agents,
            BTreeSet::from(["agent-a".into()]),
        );
        context.protected.insert("organization-resource".into());
        let rows = (0..500).map(|n| {
            let mut row = doc! {"_id":format!("protected-{n:04}"),"agent_id":"agent-a"};
            if field == "record.binding_id" {
                row.insert("record", doc! {"binding_id":"organization-resource"});
            } else {
                row.insert(field, "organization-resource");
            }
            row
        });
        db.collection::<Document>("agent_commands")
            .insert_many(rows)
            .await
            .unwrap();
        put(
            &db,
            "agent_commands",
            doc! {"_id":"zz-owned","agent_id":"agent-a"},
        )
        .await;
        let mut selected = vec![];
        let mut selector = None;
        let mut after = None;
        let mut pages = 0;
        loop {
            let page = indexed::page(
                &db,
                entry,
                std::slice::from_ref(&context),
                selector.as_ref(),
                after.as_ref(),
            )
            .await
            .unwrap();
            pages += 1;
            assert!(page.rows.len() <= 500);
            selected.extend(
                page.rows
                    .iter()
                    .filter(|row| batches::matches(entry, entry.rules[0].erasure, row, &context))
                    .cloned(),
            );
            selector = page.selector;
            after = page.after;
            if page.complete {
                break;
            }
            assert!(
                pages <= 2,
                "a protected page must advance within the matching range"
            );
        }
        assert_eq!(pages, 2);
        assert_eq!(
            selected.len(),
            1,
            "the first protected page must not hide the later owned row for {field}"
        );
        assert_eq!(selected[0].get_str("_id").unwrap(), "zz-owned");
        db.collection::<Document>("agent_commands")
            .delete_many(doc! {})
            .await
            .unwrap();
    }
    db.drop().await.unwrap();
}
