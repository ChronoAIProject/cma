//! Whole-read profiler budgets include the real proof, not just the window.
use super::*;
use futures_util::TryStreamExt;
use std::collections::BTreeMap;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set with profiler access"]
async fn prepared_public_windows_bound_all_examined_documents_and_empty_reads() {
    for history in [1_000, 10_000] {
        let fixture = Fixture::new(history).await.prepare().await;
        let db = fixture.client.database(&fixture.name);
        let profiles = db.collection::<Document>("system.profile");
        for (label, window, first, count) in [
            (
                "tail_page",
                Window::Page {
                    after: history - 200,
                    limit: 200,
                },
                history - 199,
                200,
            ),
            (
                "frozen_range",
                Window::Range {
                    after: history - 201,
                    through: history - 1,
                    limit: 200,
                },
                history - 200,
                200,
            ),
            ("prefix", Window::Prefix { through: 200 }, 1, 200),
            (
                "empty_page",
                Window::Page {
                    after: history,
                    limit: 200,
                },
                0,
                0,
            ),
        ] {
            db.run_command(doc! { "profile": 2, "slowms": 0 })
                .await
                .unwrap();
            let before = fixture.reset_commands();
            let entries = fixture.read(window, &PreparedIntegrity).await.unwrap();
            let checkouts = fixture.checkouts.load(Ordering::SeqCst) - before;
            fixture.assert_read_only();
            assert_eq!(entries.len() as u64, count, "{label}");
            for (offset, entry) in entries.iter().enumerate() {
                assert_eq!(entry.seq, first + offset as u64, "{label}");
            }
            let expected_commands = if count == 0 { 3 } else { 4 };
            assert_eq!(checkouts, expected_commands, "{label}");
            db.run_command(doc! { "profile": 0 }).await.unwrap();
            let operations: Vec<Document> = profiles
                .find(doc! {})
                .await
                .unwrap()
                .try_collect()
                .await
                .unwrap();
            let mut reads = 0;
            let mut examined = BTreeMap::<String, i64>::new();
            for operation in &operations {
                let command = operation.get_document("command").unwrap();
                if command.contains_key("find") || command.contains_key("getMore") {
                    reads += 1;
                    assert!(
                        !operation
                            .get_str("planSummary")
                            .unwrap_or("")
                            .contains("COLLSCAN"),
                        "{label}: proof and window reads require indexes"
                    );
                }
                let documents = operation
                    .get_i64("docsExamined")
                    .or_else(|_| operation.get_i32("docsExamined").map(i64::from))
                    .unwrap_or(0);
                let namespace = operation.get_str("ns").unwrap_or("command");
                let collection = namespace
                    .strip_prefix(&format!("{}.", fixture.name))
                    .unwrap_or(namespace);
                *examined.entry(collection.into()).or_default() += documents;
            }
            // Require profiler coverage, then sum every collection/operation;
            // a missing sample must not masquerade as a zero-document result.
            assert_eq!(reads, expected_commands, "{label}");
            let total: i64 = examined.values().sum();
            assert_eq!(total, count as i64 + 2, "{label}: {examined:?}");
            let budget = if count == 0 { 20 } else { 250 };
            assert!(total <= budget, "{label}: {examined:?}");
            eprintln!("public_window_profile history={history} shape={label} requested={count} checkouts={checkouts} reads={reads} total_docs_examined={total} by_collection={examined:?}");
            profiles.drop().await.unwrap();
        }
        fixture.cleanup().await;
    }
}
