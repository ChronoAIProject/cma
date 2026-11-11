use super::*;

#[tokio::test]
async fn indexed_verification_query_failures_preserve_database_and_reserve_after_reopen() {
    for table in [
        "retired_groups",
        "indexed_contexts",
        "archived_records",
        "indexed_records",
        "retired_operations",
    ] {
        let fixture = Fixture::new();
        let (context, rows, proof, _) = fixture.indexed_sources().await;
        let archive = fixture.indexed(true).await;
        let prepared = archive
            .prepare_indexed(
                proof.clone(),
                context,
                move |i| Ok(rows[i as usize].clone()),
            )
            .await
            .unwrap();
        archive
            .storage
            .call(move |db| {
                db.execute_batch(&format!("CREATE TEMP TABLE {table} (unavailable INTEGER)"))
                    .unwrap();
                Ok(())
            })
            .await
            .unwrap();
        let database = fixture.path.join("ledger.sqlite3");
        let reserve = fixture.path.join("emergency.reserve");
        let before = std::fs::read(&database).unwrap();
        let reserve_before = std::fs::read(&reserve).unwrap();
        assert!(!reserve_before.is_empty());
        assert_eq!(
            archive
                .verify_indexed(proof.clone())
                .await
                .unwrap_err()
                .code,
            "mutation_retirement_archive_invalid",
            "{table}"
        );
        assert_eq!(std::fs::read(&database).unwrap(), before, "{table}");
        assert_eq!(std::fs::read(&reserve).unwrap(), reserve_before, "{table}");
        crate::ledger_storage::reserve::require(&fixture.path).unwrap();
        archive
            .storage
            .call(move |db| {
                db.execute_batch(&format!("DROP TABLE temp.{table}"))
                    .unwrap();
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(
            archive.verify_indexed(proof.clone()).await.unwrap(),
            prepared
        );
        archive.close().await.unwrap();
        let archive = fixture.indexed(false).await;
        crate::ledger_storage::reserve::require(&fixture.path).unwrap();
        assert_eq!(std::fs::read(&reserve).unwrap(), reserve_before);
        assert_eq!(std::fs::read(&database).unwrap(), before);
        assert_eq!(archive.verify_indexed(proof).await.unwrap(), prepared);
        assert!(archive.require_ready().await.is_err());
        archive.close().await.unwrap();
    }
}
