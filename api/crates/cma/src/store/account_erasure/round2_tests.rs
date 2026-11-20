//! Real persistence regressions for the second erasure review.
use super::{tests::*, *};
use mongodb::bson::{doc, Document};

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
