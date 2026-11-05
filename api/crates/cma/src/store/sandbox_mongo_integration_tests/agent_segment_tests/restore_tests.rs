use super::*;
use mongodb::options::ReadConcern;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn coherent_restore_preserves_raw_history_projection_and_audit_capture() {
    let (client, source_name) = database("segment_backup_source").await;
    let (_, restore_name) = database("segment_backup_restored").await;
    let source = client.database(&source_name);
    segments::migrate(&source).await.unwrap();
    let store = MongoSandboxStore::open(&client, &source_name)
        .await
        .unwrap();
    let (agent, entries) = segmented(&store, 3).await;
    assert_eq!(
        segments::audit_unit(
            &client,
            &source,
            agent.id.as_str(),
            "restore-capture",
            false
        )
        .await
        .unwrap(),
        "running"
    );
    // A small synthetic backup captures original BSON, not a JSON conversion.
    // The shared snapshot also retains in-progress audit and evidence state.
    let names: Vec<_> = ["agents", "agent_log"]
        .into_iter()
        .chain(segments::COLLECTIONS.iter().copied())
        .collect();
    let mut backup = Vec::new();
    let mut session = client.start_session().snapshot(true).await.unwrap();
    for name in &names {
        let mut cursor = source
            .collection::<RawDocumentBuf>(name)
            .find(doc! {})
            .read_concern(ReadConcern::snapshot())
            .session(&mut session)
            .await
            .unwrap();
        let mut rows = Vec::new();
        while cursor.advance(&mut session).await.unwrap() {
            rows.push(cursor.deserialize_current().unwrap());
        }
        backup.push((*name, rows));
    }
    source.drop().await.unwrap();
    let restored = client.database(&restore_name);
    segments::migrate(&restored).await.unwrap();
    for (name, rows) in &backup {
        if !rows.is_empty() {
            restored
                .collection::<RawDocumentBuf>(name)
                .insert_many(rows.clone())
                .await
                .unwrap();
        }
    }
    let reopened = MongoSandboxStore::open(&client, &restore_name)
        .await
        .unwrap();
    assert_eq!(
        reopened.get_agent(&agent.id).await.unwrap(),
        Some(agent.clone())
    );
    assert_eq!(
        reopened.agent_log_range(&agent.id, 0, 8, 8).await.unwrap(),
        entries
    );
    assert!(reopened
        .agent_log_projection(&agent)
        .await
        .unwrap()
        .is_some());
    for expected in ["running", "running", "complete"] {
        assert_eq!(
            segments::audit_unit(
                &client,
                &restored,
                agent.id.as_str(),
                "restore-capture",
                false
            )
            .await
            .unwrap(),
            expected
        );
    }
    let original = &backup
        .iter()
        .find(|(name, _)| *name == "agent_log")
        .unwrap()
        .1;
    let actual: Vec<RawDocumentBuf> = restored
        .collection::<RawDocumentBuf>("agent_log")
        .find(doc! {})
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    assert_eq!(
        actual, *original,
        "restoration must preserve exact BSON bytes"
    );
    reopened
        .segment_logs
        .heads
        .delete_one(doc! {"_id":agent.id.as_str()})
        .await
        .unwrap();
    assert!(
        reopened.agent_log_range(&agent.id, 0, 8, 8).await.is_err(),
        "partial restore cannot become legacy history"
    );
    restored.drop().await.unwrap();
}
