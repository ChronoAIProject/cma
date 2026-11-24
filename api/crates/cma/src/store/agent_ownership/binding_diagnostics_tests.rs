use super::*;
use futures_util::{future::BoxFuture, FutureExt};

async fn with_fixture(test: impl for<'a> FnOnce(&'a MongoAgentOwnership) -> BoxFuture<'a, ()>) {
    let store = fixture().await;
    let outcome = std::panic::AssertUnwindSafe(test(&store))
        .catch_unwind()
        .await;
    store.database.drop().await.unwrap();
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

async fn fixture() -> MongoAgentOwnership {
    let uri =
        std::env::var("CHRONO_TEST_MONGODB_URI").expect("use an owned disposable Mongo fixture");
    let client = Client::with_uri_str(uri).await.unwrap();
    let name = format!("cma_binding_observation_{}", uuid::Uuid::new_v4().simple());
    let store = MongoAgentOwnership::open(&client, &name).await.unwrap();
    // Keep expired rows present so expiry is proved independently of TTL deletion.
    store
        .members
        .drop_index("agent_manager_members_expiry")
        .await
        .unwrap();
    store
        .members
        .create_index(
            mongodb::IndexModel::builder()
                .keys(doc! {"expires_at":1})
                .options(
                    mongodb::options::IndexOptions::builder()
                        .name("agent_manager_members_expiry".to_owned())
                        .build(),
                )
                .build(),
        )
        .await
        .unwrap();
    store
}

fn member_row(id: &str, expires: i64) -> Document {
    doc! {
        "_id":id,"document_schema_version":1_i64,"protocol":i64::from(PROTOCOL_VERSION),
        "relay_address":"http://replica.example.test/","draining":false,
        "owned_agents":0_i64,"max_owned_agents":16_i64,
        "expires_at":mongodb::bson::DateTime::from_millis(expires),
    }
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn bounded_membership_is_read_only_and_excludes_expired_without_ttl_cleanup() {
    with_fixture(|store| {
        Box::pin(async move {
            let expires = store.server_now().await.unwrap() + 60_000;
            let mut rows: Vec<_> = (0..20)
                .map(|i| member_row(&format!("member-{i}"), expires))
                .collect();
            rows.push(member_row("expired", 1));
            rows[0].insert("irrelevant_payload", "x".repeat(512 * 1024));
            store.members.insert_many(rows).await.unwrap();
            let before: Vec<Document> = store
                .members
                .find(doc! {})
                .sort(doc! {"_id":1})
                .await
                .unwrap()
                .try_collect()
                .await
                .unwrap();
            let page = store.binding_diagnostic_members().await.unwrap();
            assert_eq!(page.members.len(), MAX_REPORTERS);
            assert!(page.truncated);
            assert!(!page
                .members
                .iter()
                .any(|member| member.instance_id == "expired"));
            store
                .check_binding_diagnostic_members("member-0", "member-1")
                .await
                .unwrap();
            assert!(store
                .check_binding_diagnostic_members("member-0", "expired")
                .await
                .is_err());
            assert!(store
                .check_binding_diagnostic_members("missing", "member-1")
                .await
                .is_err());
            let after: Vec<Document> = store
                .members
                .find(doc! {})
                .sort(doc! {"_id":1})
                .await
                .unwrap()
                .try_collect()
                .await
                .unwrap();
            assert_eq!(before, after);
            store
                .members
                .update_one(doc! {"_id":"member-1"}, doc! {"$set":{"draining":true}})
                .await
                .unwrap();
            assert!(store
                .check_binding_diagnostic_members("member-0", "member-1")
                .await
                .is_err());
            store
                .members
                .update_one(
                    doc! {"_id":"member-1"},
                    doc! {"$set":{"draining":false,"protocol":i64::from(PROTOCOL_VERSION)+1}},
                )
                .await
                .unwrap();
            assert!(store
                .check_binding_diagnostic_members("member-0", "member-1")
                .await
                .is_err());
            assert!(store
                .check_binding_diagnostic_members("member-1", "member-0")
                .await
                .is_err());
        })
    })
    .await;
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn projected_current_owners_refuse_unknown_schema_and_large_fields_without_writes() {
    with_fixture(|store| Box::pin(async move {
    let sandboxes = store.database.collection::<Document>("sandboxes");
    sandboxes.insert_many([
        doc! {"_id":"one","document_schema_version":1_i64,"owner":"owner-a","record":{"ignored":"x".repeat(512*1024)}},
        doc! {"_id":"two","document_schema_version":3_i64,"owner":"owner-b"},
        doc! {"_id":"three","document_schema_version":4_i64,"owner":"owner-c"},
    ]).await.unwrap();
    let ids: Vec<_> = ["one","two","three","deleted"].into_iter().map(|id| SandboxId(id.into())).collect();
    let before: Vec<Document> = sandboxes.find(doc! {}).sort(doc! {"_id":1}).await.unwrap().try_collect().await.unwrap();
    let owners = current_owners(&sandboxes, &ids).await.unwrap();
    assert_eq!(owners.len(), 3);
    assert_eq!(owners.get(&SandboxId("two".into())), Some(&SubjectId("owner-b".into())));
    assert!(!owners.contains_key(&SandboxId("deleted".into())));
    let after: Vec<Document> = sandboxes.find(doc! {}).sort(doc! {"_id":1}).await.unwrap().try_collect().await.unwrap();
    assert_eq!(before, after);
    sandboxes.update_one(doc! {"_id":"two"},doc! {"$set":{"document_schema_version":99_i64}}).await.unwrap();
    assert!(current_owners(&sandboxes, &ids).await.is_err());
    sandboxes.update_one(doc! {"_id":"two"},doc! {"$set":{"document_schema_version":3_i64,"owner":"界".repeat(10_000)}}).await.unwrap();
    assert!(current_owners(&sandboxes, &ids).await.is_err());
    sandboxes.update_one(doc! {"_id":"two"},doc! {"$set":{"owner":["owner-b"]}}).await.unwrap();
    assert!(current_owners(&sandboxes, &ids).await.is_err());
    let excess = vec![SandboxId("one".into()); MAX_REPORTERS * MAX_HOLDS_PER_REPORTER + 1];
    assert!(current_owners(&sandboxes, &excess).await.is_err());
    })).await;
}
