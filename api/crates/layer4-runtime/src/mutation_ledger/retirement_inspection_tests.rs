use super::*;
use crate::mutation_ledger::tests::Fixture;

#[tokio::test]
async fn retirement_inventory_query_failures_preserve_database_and_reserve_after_reopen() {
    for table in [
        "metadata",
        "accounting",
        "operations",
        "authority",
        "cleanup_budget",
        "scope_budget",
        "logical_processes",
    ] {
        let fixture = Fixture::new();
        let ledger = fixture.open(true).await;
        tests::finish(&ledger, 0).await;
        let digest = tests::inventory(&ledger).await.2;
        ledger
            .storage
            .call(move |db| {
                // Real query errors, confined to this connection's temporary
                // schema: no damaged durable state can explain reserve loss.
                db.execute_batch(&format!("CREATE TEMP TABLE {table} (unavailable INTEGER)"))
                    .unwrap();
                Ok(())
            })
            .await
            .unwrap();
        let database = fixture.database().join("ledger.sqlite3");
        let reserve = fixture.database().join("emergency.reserve");
        let before = std::fs::read(&database).unwrap();
        let reserve_before = std::fs::read(&reserve).unwrap();
        assert!(!reserve_before.is_empty());
        let mut cursor = None;
        let error = loop {
            match ledger.retirement_inventory(cursor).await {
                Ok(page) => {
                    assert!(page.next.is_some(), "shadowed table was not inspected");
                    cursor = page.next;
                }
                Err(error) => break error,
            }
        };
        assert_eq!(error.code, "mutation_ledger_invalid", "{table}");
        assert_eq!(std::fs::read(&database).unwrap(), before, "{table}");
        assert_eq!(std::fs::read(&reserve).unwrap(), reserve_before, "{table}");
        crate::ledger_storage::reserve::require(&fixture.database()).unwrap();
        ledger
            .storage
            .call(move |db| {
                db.execute_batch(&format!("DROP TABLE temp.{table}"))
                    .unwrap();
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(tests::inventory(&ledger).await.2, digest);
        ledger.close().await.unwrap();
        let ledger = fixture.open(false).await;
        crate::ledger_storage::reserve::require(&fixture.database()).unwrap();
        assert_eq!(std::fs::read(&reserve).unwrap(), reserve_before);
        assert_eq!(std::fs::read(&database).unwrap(), before);
        assert_eq!(tests::inventory(&ledger).await.2, digest);
        ledger.close().await.unwrap();
    }
}
