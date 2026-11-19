//! Count the actual compatibility-page adapter, including every proof query.
use super::*;
use futures_util::TryStreamExt;

#[tokio::test]
#[ignore = "requires the disposable replica-set runner with profiler access"]
async fn prepared_public_page_projection_bounds_all_commands_and_documents() {
    for history in [1_000, 10_000] {
        let f = Fixture::new(history).await.prepare().await;
        let agent = f.store.get_agent(&f.id).await.unwrap().unwrap();
        let db = f.client.database(&f.name);
        let profiles = db.collection::<Document>("system.profile");
        for after in [0, history - 200, history] {
            db.run_command(doc! {"profile":2,"slowms":0}).await.unwrap();
            let before = f.reset_commands();
            let page = f
                .store
                .agent_log_page_projection(&agent, after, 200)
                .await
                .unwrap()
                .unwrap();
            let commands = f.commands.lock().unwrap().len();
            let checkouts = f.checkouts.load(Ordering::SeqCst) - before;
            f.assert_read_only();
            db.run_command(doc! {"profile":0}).await.unwrap();
            let operations: Vec<Document> = profiles
                .find(doc! {})
                .await
                .unwrap()
                .try_collect()
                .await
                .unwrap();
            let examined: i64 = operations
                .iter()
                .map(|op| {
                    op.get_i64("docsExamined")
                        .or_else(|_| op.get_i32("docsExamined").map(i64::from))
                        .unwrap_or(0)
                })
                .sum();
            let reads = operations
                .iter()
                .filter(|op| {
                    op.get_document("command")
                        .is_ok_and(|cmd| cmd.contains_key("find") || cmd.contains_key("getMore"))
                })
                .count();
            assert_eq!(reads, commands, "profiler must cover every driver command");
            assert_eq!(checkouts, commands as u64);
            assert_eq!(page.len(), if after == history { 0 } else { 200 });
            assert_eq!(examined, page.len() as i64 + 3);
            assert!(examined <= if page.is_empty() { 20 } else { 250 });
            for (offset, row) in page.iter().enumerate() {
                assert_eq!(row.seq, after + offset as u64 + 1);
            }
            eprintln!("public_projection history={history} after={after} rows={} commands={commands} checkouts={checkouts} docs_examined={examined}", page.len());
            profiles.drop().await.unwrap();
        }
        f.store
            .agent_log
            .delete_one(doc! {"agent_id":f.id.as_str(),"seq":100_i64})
            .await
            .unwrap();
        let before = f.reset_commands();
        assert!(f
            .store
            .agent_log_page_projection(&agent, 0, 200)
            .await
            .is_err());
        f.assert_read_only();
        assert!(f.checkouts.load(Ordering::SeqCst) > before);
        f.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "requires the disposable replica-set runner"]
async fn public_projection_keeps_unprepared_fallback_and_refuses_quarantine() {
    let mut f = Fixture::new(1000).await.prepare().await;
    let agent = f.store.get_agent(&f.id).await.unwrap().unwrap();
    f.store = MongoSandboxStore::open(&f.client, &f.name).await.unwrap();
    assert!(f
        .store
        .agent_log_page_projection(&agent, 0, 200)
        .await
        .unwrap()
        .is_none());
    f.store
        .agents
        .update_one(
            doc! {"_id":f.id.as_str()},
            doc! {"$set":{QUARANTINE_FIELD:{"reason":"fixture_corruption"}}},
        )
        .await
        .unwrap();
    assert!(f
        .store
        .agent_log_page_projection(&agent, 0, 200)
        .await
        .is_err());
    f.cleanup().await;
}
