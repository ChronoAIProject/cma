use super::*;
use crate::store::agent_log_integrity::TransactionLogError;
use mongodb::bson::{
    oid::ObjectId, DateTime, Decimal128, JavaScriptCodeWithScope, Regex, Timestamp,
};
use segments::capture::{capture, CaptureMode};

fn malformed_types() -> Vec<Bson> {
    vec![
        Bson::Null,
        Bson::String("6".into()),
        Bson::Int32(6),
        Bson::Double(6.0),
        Bson::Double(6.5),
        Bson::Double(f64::NAN),
        Bson::Double(f64::INFINITY),
        Bson::Double(f64::NEG_INFINITY),
        Bson::Decimal128("6".parse::<Decimal128>().unwrap()),
        Bson::Decimal128("6.5".parse::<Decimal128>().unwrap()),
        Bson::Decimal128("NaN".parse::<Decimal128>().unwrap()),
        Bson::Decimal128("Infinity".parse::<Decimal128>().unwrap()),
        Bson::Array(vec![]),
        Bson::Array(vec![Bson::Int64(6)]),
        Bson::Array(vec![Bson::Array(vec![Bson::Int64(6)])]),
        Bson::Document(doc! {"value":6_i64}),
        Bson::Binary(digest::binary(vec![6])),
        Bson::Undefined,
        Bson::ObjectId(ObjectId::from_bytes([6; 12])),
        Bson::Boolean(true),
        Bson::DateTime(DateTime::from_millis(6)),
        Bson::RegularExpression(Regex {
            pattern: "six".try_into().unwrap(),
            options: "".try_into().unwrap(),
        }),
        serde_json::from_value(serde_json::json!({"$dbPointer":{
            "$ref":"synthetic", "$id":{"$oid":"060606060606060606060606"}
        }}))
        .unwrap(),
        Bson::JavaScriptCode("6".into()),
        Bson::Symbol("6".into()),
        Bson::JavaScriptCodeWithScope(JavaScriptCodeWithScope {
            code: "value".into(),
            scope: doc! {"value":6_i64},
        }),
        Bson::Timestamp(Timestamp {
            time: 6,
            increment: 1,
        }),
        Bson::MinKey,
        Bson::MaxKey,
    ]
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn every_non_int64_bson_type_is_refused_outside_the_requested_page() {
    let (client, name) = database("seq_types").await;
    let db = client.database(&name);
    schema::migrate(&db).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, entries) = segmented(&store, 3).await;
    certify(&store, &agent, &entries).await;
    let original = log_document(&entries[5]).unwrap();
    let filter = doc! {"_id":original.get("_id").unwrap()};
    let raw = store.agent_log.clone_with_type::<RawDocumentBuf>();
    for seq in malformed_types() {
        let mut row = original.clone();
        row.insert("seq", seq.clone());
        store
            .agent_log
            .replace_one(filter.clone(), row)
            .bypass_document_validation(true)
            .await
            .unwrap();
        let before = raw.find_one(filter.clone()).await.unwrap().unwrap();
        // The changed row is in another sealed segment, beyond every point
        // requested here and outside capture's active-tail scan.
        let mut visits = 0;
        let result = crate::store::agent_log_store::visit_page(
            &store,
            &agent.id,
            0,
            2,
            2,
            true,
            &mut |_| {
                visits += 1;
                Ok(())
            },
        )
        .await;
        assert!(refused(&result), "{seq:?}: {result:?}");
        assert_eq!(visits, 0, "{seq:?}");
        for point in [1, 2, 9] {
            let result = store.agent_log_entry(&agent.id, point).await;
            assert!(refused(&result), "{seq:?} point={point}: {result:?}");
        }
        // Zero is the existing argument sentinel; it selects no source.
        assert_eq!(store.agent_log_entry(&agent.id, 0).await.unwrap(), None);
        let mut session = client.start_session().snapshot(true).await.unwrap();
        assert!(
            matches!(
                capture(&store, &agent, &mut session, CaptureMode::Snapshot).await,
                Err(TransactionLogError::Domain(SandboxStoreError::Corrupt(_)))
            ),
            "{seq:?}"
        );
        drop(session);
        assert_eq!(
            raw.find_one(filter.clone())
                .await
                .unwrap()
                .unwrap()
                .as_bytes(),
            before.as_bytes()
        );
        store
            .agent_log
            .replace_one(filter.clone(), original.clone())
            .await
            .unwrap();
        assert_eq!(
            store.agent_log_range(&agent.id, 0, 8, 20).await.unwrap(),
            entries
        );
        eprintln!("malformed_seq_type {seq:?}: refused before visitors, points and capture; raw BSON retained");
    }
    db.drop().await.unwrap();
}
