//! Include authorization, binding, source and final fences in the page budget.
use super::*;
use futures_util::TryStreamExt;

#[tokio::test]
#[ignore = "requires the disposable replica-set runner with profiler access"]
async fn prepared_legacy_public_pages_include_all_authority_and_binding_reads() {
    for rows in [1_000, 10_000] {
        let mut f = Fixture::semantic(rows).await;
        f.store = f
            .store
            .prepare_bounded_admission(std::slice::from_ref(&f.agent.id))
            .await
            .unwrap();
        let db = f.client.database(&f.name);
        let profiles = db.collection::<Document>("system.profile");
        for route in ["compatibility", "events_v2", "events_v2_single_fence"] {
            let app = f.app(false, route == "events_v2_single_fence");
            let reads = f.reads(false);
            db.run_command(doc! {"profile":2,"slowms":0}).await.unwrap();
            f.meter.clear();
            if route == "compatibility" {
                let (page, next) = reads
                    .log_page_authorized(&context(f.agent.owner.clone()), None, &f.agent.id, 0, 200)
                    .await
                    .unwrap();
                assert_eq!(page.len(), 200);
                assert_eq!(next, Some(200));
            } else {
                let bytes = f
                    .request(
                        &app,
                        &format!("/api/v2/agents/{}/events/snapshot?limit=200", f.agent.id),
                    )
                    .await;
                let page: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(page["items"].as_array().unwrap().len(), 200);
            }
            let actual = f.meter.stats.lock().unwrap().clone();
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
            let commands = actual.commands.values().sum::<u64>();
            assert_eq!(commands, actual.checkouts);
            let driver_reads: u64 = actual
                .commands
                .iter()
                .filter(|(name, _)| {
                    name.ends_with("/find")
                        || name.ends_with("/aggregate")
                        || name.ends_with("/getMore")
                })
                .map(|(_, count)| count)
                .sum();
            let profiled_reads = operations
                .iter()
                .filter(|op| {
                    op.get_document("command").is_ok_and(|cmd| {
                        cmd.contains_key("find")
                            || cmd.contains_key("aggregate")
                            || cmd.contains_key("getMore")
                    })
                })
                .count();
            assert_eq!(
                driver_reads, profiled_reads as u64,
                "every data read must be profiled"
            );
            assert!(examined <= 250, "{route}: {examined} examined documents");
            eprintln!("public_page_complete rows={rows} route={route} page=200 commands={commands} checkouts={} docs_examined={examined} returned_docs={} returned_bytes={}", actual.checkouts, actual.docs, actual.bytes);
            profiles.drop().await.unwrap();
        }
        f.client.database(&f.name).drop().await.unwrap();
    }
}
