//! A durable cursor advances bounded shared reference pages across worker loss.
use super::*;
use crate::store::trigger_definitions::integration_tests::seeded;
use mongodb::{
    bson::{doc, Document},
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
    Client,
};
use std::sync::{Arc, Mutex};

fn number(row: &Document, field: &str) -> i64 {
    row.get_i64(field)
        .or_else(|_| row.get_i32(field).map(i64::from))
        .unwrap()
}
async fn explain_page(db: &Database, command: &Document) {
    let mut query = Document::new();
    for field in ["find", "filter", "projection", "limit", "hint", "sort"] {
        if let Some(value) = command.get(field) {
            query.insert(field, value.clone());
        }
    }
    let plan = db
        .run_command(doc! {"explain":query,"verbosity":"executionStats"})
        .await
        .unwrap();
    let winning = plan
        .get_document("queryPlanner")
        .unwrap()
        .get("winningPlan")
        .unwrap();
    let stats = plan.get_document("executionStats").unwrap();
    assert!(
        format!("{winning:?}").contains("IXSCAN")
            || (number(stats, "nReturned") == 0
                && number(stats, "totalKeysExamined") == 0
                && number(stats, "totalDocsExamined") == 0),
        "nonempty reference pages must use an index: {plan:?}"
    );
    assert!(
        number(stats, "totalKeysExamined") <= 501,
        "keyset pages must not rescan their prefixes: {command:?} {plan:?}"
    );
    assert!(
        number(stats, "totalDocsExamined") <= 501,
        "reference page must examine bounded documents: {command:?} {plan:?}"
    );
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn interrupted_reference_discovery_resumes_without_restarting_or_erasing_early() {
    let seed = seeded("round3_discovery_resume").await;
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let observed = Arc::new(Mutex::new(Vec::<Document>::new()));
    let commands = observed.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            if event.command_name == "find" {
                commands.lock().unwrap().push(event.command);
            }
        }
    }));
    let client = Client::with_options(options).unwrap();
    let db = client.database(&seed.database);
    db.collection::<Document>("agents").insert_many((0..1301).map(|n| doc! {"_id":format!("agt_foreign_{n:05}"),"owner":"visitor","record":{"profile":{"id":seed.target.agent_profile.id.as_str(),"revision":1_i64}}})).await.unwrap();
    let mut second = db
        .collection::<Document>("agent_profile_revisions")
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap();
    second.insert(
        "_id",
        format!("{}:0000000002", seed.target.agent_profile.id),
    );
    second.insert("revision", 2_i64);
    second
        .get_document_mut("record")
        .unwrap()
        .insert("revision", 2_i64);
    db.collection::<Document>("agent_profile_revisions")
        .insert_one(second)
        .await
        .unwrap();
    db.collection::<Document>("agents").insert_one(doc! {"_id":mongodb::bson::oid::ObjectId::new(),"owner":"visitor","record":{"profile":{"id":seed.target.agent_profile.id.as_str(),"revision":2_i64}}}).await.unwrap();
    let expected = profile_refs::retained(&db, &BTreeSet::from([seed.owner.to_string()]))
        .await
        .unwrap();
    assert!(
        expected.contains(&format!("{}:0000000002", seed.target.agent_profile.id)),
        "the trailing BSON identity is an independent retained pin"
    );
    let mut cursor = Cursor::default();
    let mut inventory = ErasureInventory::default();
    let mut scans = Vec::new();
    let mut calls_with_scan = 0;
    for _ in 0..100 {
        observed.lock().unwrap().clear();
        let batch = erase_records(
            &db,
            &[Target {
                subject: &seed.owner,
                inventory: &inventory,
            }],
            &mut cursor,
            "2026-11-14T00:00:00Z",
        )
        .await
        .unwrap();
        progress::acknowledge(&mut inventory, &batch, seed.owner.as_str());
        let reference_pages: Vec<_> = observed
            .lock()
            .unwrap()
            .iter()
            .filter(|command| {
                command.get_document("projection").is_ok_and(|projection| {
                    projection.keys().any(|key| {
                        key.starts_with("record.profile")
                            || key.starts_with("record.draft.")
                            || key.starts_with("record.content.")
                            || key.starts_with("record.revisions.")
                            || key.starts_with("record.source.")
                            || key.starts_with("record.profiles.")
                            || key.starts_with("bot.profiles.")
                            || key == "settings.defaults"
                            || key == "record.forked_from"
                            || (key == "owner" && projection.len() == 1)
                    })
                })
            })
            .cloned()
            .collect();
        assert!(
            reference_pages.len() <= 1,
            "reference discovery has one page of work per call"
        );
        for command in &reference_pages {
            assert_eq!(number(command, "limit"), 500);
            explain_page(&db, command).await;
        }
        let reads: Vec<_> = observed
            .lock()
            .unwrap()
            .iter()
            .filter(|command| {
                command.get_str("find").ok() == Some("agents")
                    && command
                        .get_document("projection")
                        .is_ok_and(|p| p.contains_key("record.profile"))
            })
            .cloned()
            .collect();
        if !reads.is_empty() {
            calls_with_scan += 1;
            assert_eq!(reads.len(), 1, "one discovery source page per invocation");
            let command = &reads[0];
            assert_eq!(
                command
                    .get_i64("limit")
                    .or_else(|_| command.get_i32("limit").map(i64::from))
                    .ok(),
                Some(500),
                "every reference query must have a bounded page"
            );
            assert_eq!(command.get_document("sort").unwrap(), &doc! {"_id":1});
            scans.push(command.get_document("filter").unwrap().clone());
            if cursor.retained_profiles.is_none() {
                assert!(batch.deleted.is_empty() && batch.redacted.is_empty());
            }
        }
        if let Some(retained) = &cursor.retained_profiles {
            assert_eq!(retained, &expected);
            break;
        }
        let bytes = serde_json::to_vec(&cursor).unwrap();
        cursor = serde_json::from_slice(&bytes).unwrap();
        // Test-only elapsed-time controls are deliberately not persisted.
        cursor.test_drain_seconds = Some(0);
    }
    assert_eq!(calls_with_scan, 3);
    assert!(
        scans.windows(2).all(|pair| pair[0] != pair[1]),
        "resume must advance the last identity rather than repeat a page"
    );
    assert_eq!(cursor.retained_profiles.as_ref(), Some(&expected));
    assert_eq!(
        db.collection::<Document>("agents")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1302
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn owned_keyset_never_skips_a_nonstring_identity_after_string_pages() {
    use mongodb::bson::{oid::ObjectId, Bson};
    let seed = seeded("round3_owned_mixed_identity").await;
    let db = seed.client.database(&seed.database);
    let profiles = db.collection::<Document>("agent_profiles");
    profiles
        .insert_many((0..1001).map(
            |n| doc! {"_id":format!("agp_page_{n:05}"),"owner":seed.owner.as_str(),"record":{}},
        ))
        .await
        .unwrap();
    let malformed = ObjectId::new();
    profiles
        .insert_one(doc! {"_id":malformed,"owner":seed.owner.as_str(),"record":{}})
        .await
        .unwrap();
    let error = profile_refs::retained(&db, &BTreeSet::from([seed.owner.to_string()]))
        .await
        .unwrap_err();
    assert!(
        matches!(error,Error::Blocked(AttributedBlock {subject_hash,reason:ErasureBlock::CorruptRecord {..}}) if subject_hash == layer2_core::account_deletion::subject_hash(&seed.owner)),
        "invalid owned identities must remain visible after every keyset boundary"
    );
    assert_eq!(profiles.count_documents(doc! {}).await.unwrap(), 1003);
    profiles
        .delete_one(doc! {"_id":Bson::ObjectId(malformed)})
        .await
        .unwrap();
    assert!(
        profile_refs::retained(&db, &BTreeSet::from([seed.owner.to_string()]))
            .await
            .unwrap()
            .is_empty()
    );
    db.drop().await.unwrap();
}
