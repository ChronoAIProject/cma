use super::*;

fn plan_has_stage(value: &mongodb::bson::Bson, stage: &str) -> bool {
    match value {
        mongodb::bson::Bson::Document(document) => {
            document.get_str("stage").ok() == Some(stage)
                || document.values().any(|value| plan_has_stage(value, stage))
        }
        mongodb::bson::Bson::Array(values) => {
            values.iter().any(|value| plan_has_stage(value, stage))
        }
        _ => false,
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn standing_queries_use_indexes_and_project_only_authority() {
    use mongodb::{
        event::command::CommandEvent, event::EventHandler, options::ClientOptions, Client,
    };
    use std::sync::{Arc, Mutex};
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let commands = Arc::new(Mutex::new(Vec::new()));
    let observed = commands.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            if event.command_name == "find" && event.command.get_str("find").ok() == Some(RECORDS) {
                observed.lock().unwrap().push(event.command);
            }
        }
    }));
    let client = Client::with_options(options).unwrap();
    let db = client.database(&format!(
        "cma_acd_projection_{}",
        uuid::Uuid::new_v4().simple()
    ));
    let store = MongoAccountDeletionStore::open(&db).await.unwrap();
    let owner = SubjectId::new("owner");
    store.request(&owner, "original", NOW).await.unwrap();
    let array_owner = SubjectId::new("mixed-array-owner");
    let array_record = store.request(&array_owner, "array", NOW).await.unwrap();
    db.collection::<Document>(RECORDS)
        .update_one(
            doc! {"_id":array_record.id.as_str()},
            doc! {"$unset":{"active_subject_hash":""},"$set":{"status":["completed","running"]}},
        )
        .await
        .unwrap();
    let mut history = Vec::new();
    for index in 0..1024 {
        history.push(doc! {
            "_id":format!("historical-{index}"),
            "subject_hash":subject_hash(&SubjectId::new(format!("historical-{index}"))),
            "status":"completed",
            "request_keys":[format!("historical-key-{index}")],
        });
    }
    db.collection::<Document>(RECORDS)
        .insert_many(history)
        .await
        .unwrap();
    commands.lock().unwrap().clear();
    assert_eq!(
        store.active_hashes().await.unwrap(),
        BTreeSet::from([subject_hash(&owner), subject_hash(&array_owner)])
    );
    assert!(store.erasing(&owner).await.unwrap());
    let mut session = db.client().start_session().await.unwrap();
    crate::store::sandbox_mongo::start_transaction(&mut session)
        .await
        .unwrap();
    assert!(erasing_in_session(&db, &owner, &mut session).await.unwrap());
    session.abort_transaction().await.unwrap();
    let reads = commands.lock().unwrap().clone();
    assert!(reads.len() >= 3);
    for read in reads {
        let projection = read.get_document("projection").unwrap();
        assert!(!projection.contains_key("inventory"));
        assert!(!projection.contains_key("request_keys"));
        let mut find = Document::new();
        for key in [
            "find",
            "filter",
            "projection",
            "hint",
            "limit",
            "singleBatch",
        ] {
            if let Some(value) = read.get(key) {
                find.insert(key, value.clone());
            }
        }
        let explain = db
            .run_command(doc! {"explain":find,"verbosity":"executionStats"})
            .await
            .unwrap();
        let plan = explain
            .get_document("queryPlanner")
            .unwrap()
            .get("winningPlan")
            .unwrap();
        assert!(plan_has_stage(plan, "IXSCAN"), "{explain:?}");
        assert!(!plan_has_stage(plan, "COLLSCAN"), "{explain:?}");
        let statistics = explain.get_document("executionStats").unwrap();
        for counter in ["totalDocsExamined", "totalKeysExamined"] {
            let examined = statistics
                .get_i64(counter)
                .or_else(|_| statistics.get_i32(counter).map(i64::from))
                .unwrap();
            assert!(
                examined < 10,
                "standing must not scan tombstones: {explain:?}"
            );
        }
    }
    let indexes = db
        .collection::<Document>(RECORDS)
        .list_indexes()
        .await
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
    assert!(indexes.iter().any(|index| {
        index
            .options
            .as_ref()
            .and_then(|options| options.name.as_deref())
            == Some("account_deletions_status_subject")
            && index.keys == doc! {"status":1,"subject_hash":1}
    }));
    assert!(indexes.iter().any(|index| {
        index.options.as_ref().is_some_and(|options| {
            options.name.as_deref() == Some("account_deletions_array_status_subject")
                && options.partial_filter_expression == Some(doc! {"status":{"$type":"array"}})
                && !options.unique.unwrap_or(false)
                && options.expire_after.is_none()
        }) && index.keys == doc! {"subject_hash":1}
    }));
    db.drop().await.unwrap();
}
