use super::*;
use layer2_core::sandbox::store::SandboxStoreError;
use mongodb::bson::Bson;

mod contexts;
mod types;

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn page_matrix_refuses_missing_null_and_string_sequences_before_visiting() {
    let (client, name) = database("malformed_seq").await;
    let db = client.database(&name);
    schema::migrate(&db).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, entries) = segmented(&store, 3).await;
    let mut failures = Vec::new();
    for (shape, seq) in [
        ("missing", None),
        ("null", Some(Bson::Null)),
        ("string", Some(Bson::String("malformed".into()))),
    ] {
        let mut row = log_document(&entries[1]).unwrap();
        row.insert("_id", format!("malformed-source:{shape}"));
        match seq {
            Some(seq) => {
                row.insert("seq", seq);
            }
            None => {
                row.remove("seq");
            }
        }
        store
            .agent_log
            .insert_one(row.clone())
            .bypass_document_validation(true)
            .await
            .unwrap();
        for (range, after, through) in [("sealed", 0, 2), ("active", 6, 8), ("empty", 8, 8)] {
            let result = store.agent_log_range(&agent.id, after, through, 20).await;
            if !refused(&result) {
                failures.push(format!("{shape}/{range}: range returned {result:?}"));
            }
            for predecessor in [false, true] {
                let mut delivered = 0;
                let result = crate::store::agent_log_store::visit_page(
                    &store,
                    &agent.id,
                    after,
                    through,
                    20,
                    predecessor,
                    &mut |_| {
                        delivered += 1;
                        Ok(())
                    },
                )
                .await;
                eprintln!("malformed_seq shape={shape} range={range} predecessor={predecessor} refused={} visitors={delivered}", refused(&result));
                if !refused(&result) || delivered != 0 {
                    failures.push(format!(
                        "{shape}/{range}/{predecessor}: {result:?}, {delivered} visitors"
                    ));
                }
            }
        }
        let filter = doc! {"_id":row.get_str("_id").unwrap()};
        assert_eq!(
            store.agent_log.find_one(filter.clone()).await.unwrap(),
            Some(row)
        );
        assert_eq!(
            collection(&store, AUDITS)
                .count_documents(doc! {})
                .await
                .unwrap(),
            0
        );
        store.agent_log.delete_one(filter).await.unwrap();
        assert_eq!(
            store.agent_log_range(&agent.id, 0, 8, 20).await.unwrap(),
            entries
        );
    }
    db.drop().await.unwrap();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn refused<T>(result: &Result<T, SandboxStoreError>) -> bool {
    matches!(
        result,
        Err(SandboxStoreError::Corrupt(_) | SandboxStoreError::SequenceConflict)
    )
}
