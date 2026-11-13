use super::*;
use segments::audit::{self, test_support as hooks};

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn malformed_durable_promotion_refuses_resume_without_writing() {
    let (client, name) = database("audit_validation").await;
    let db = client.database(&name);
    schema::migrate(&db).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, _) = super::audit_tests::conversation(&store, 180, 1).await;
    let claim = hooks::claim(&client, &db, agent.id.as_str(), "validation")
        .await
        .unwrap();
    let mut valid = hooks::collect(&client, &db, claim).await.unwrap();
    valid.insert("expires_at_ms", 0_i64);
    let audits = db.collection::<Document>(AUDITS);
    let key = doc! {"_id":valid.get_str("_id").unwrap()};
    let head = collection(&store, HEADS).find_one(doc! {}).await.unwrap();
    let source = store
        .agent_log
        .find(doc! {})
        .sort(doc! {"seq":1})
        .await
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
    let evidence = collection(&store, EVIDENCE)
        .find(doc! {})
        .sort(doc! {"_id":1})
        .await
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
    for path in [
        "completed.next_ordinal",
        "stage.completed.next_ordinal",
        "stage.completed.checked_entries",
        "stage.completed.checked_bytes",
        "captured_head.revision",
        "stage.completed.out_of_bounds_bytes",
    ] {
        let mut damaged = valid.clone();
        let (parent, field) = path.rsplit_once('.').unwrap();
        let mut row = &mut damaged;
        for name in parent.split('.') {
            row = row.get_document_mut(name).unwrap();
        }
        if field == "out_of_bounds_bytes" {
            let total = valid
                .get_document("captured_head")
                .unwrap()
                .get_i64("lifetime_bytes")
                .unwrap();
            row.insert("checked_bytes", total + 1);
        } else {
            let value = i32::try_from(row.get_i64(field).unwrap()).unwrap();
            row.insert(field, value);
        }
        audits
            .replace_one(key.clone(), damaged.clone())
            .await
            .unwrap();
        assert!(
            matches!(
                audit::run_unit(&client, &db, agent.id.as_str(), "validation", false).await,
                Err(layer2_core::sandbox::store::SandboxStoreError::Corrupt(_))
            ),
            "accepted malformed durable field: {path}"
        );
        assert!(
            audit::install(&client, &db, agent.id.as_str(), "validation")
                .await
                .is_err()
        );
        assert_eq!(audits.find_one(key.clone()).await.unwrap(), Some(damaged));
        assert_eq!(
            collection(&store, HEADS).find_one(doc! {}).await.unwrap(),
            head
        );
        assert_eq!(
            collection(&store, EVIDENCE)
                .find(doc! {})
                .sort(doc! {"_id":1})
                .await
                .unwrap()
                .try_collect::<Vec<_>>()
                .await
                .unwrap(),
            evidence
        );
        assert_eq!(
            store
                .agent_log
                .find(doc! {})
                .sort(doc! {"seq":1})
                .await
                .unwrap()
                .try_collect::<Vec<_>>()
                .await
                .unwrap(),
            source
        );
    }
    audits.replace_one(key, valid).await.unwrap();
    assert_eq!(
        audit::run_unit(&client, &db, agent.id.as_str(), "validation", false)
            .await
            .unwrap(),
        "complete"
    );
    assert_eq!(
        audit::install(&client, &db, agent.id.as_str(), "validation")
            .await
            .unwrap(),
        "installed"
    );
    db.drop().await.unwrap();
}
