use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn terminal_usage_rejects_cached_compute_tails_for_daily_cold_and_bucketed_ranges() {
    let store = fixture()
        .await
        .with_view_policy(crate::config::report_views::ViewPolicy {
            allocation_rollups_enabled: true,
            ..Default::default()
        });
    store.initialize_allocation_buckets(true).await.unwrap();
    let day = (now() / 86_400 - 3) * 86_400;
    let mut failed = state(1, day);
    let prior = failed.clone();
    failed.status = "failed".into();
    for offset in 0..2 {
        let from = day + offset * 86_400;
        let range = Range {
            from,
            to: from + 86_400,
        };
        let legacy = allocation::project(&prior, &[], range);
        assert_eq!(legacy.period.uptime_seconds, 86_400.0);
        store
            .collection(super::super::super::allocation_buckets::COLLECTION)
            .insert_one(
                ReportingStore::allocation_bucket_fixture_document(&prior, &[], range).unwrap(),
            )
            .await
            .unwrap();
        store
            .collection("sandbox_usage_rollups")
            .insert_one(doc! {
                "_id": format!("{}:{from}", failed.sandbox_id),
                "document_schema_version": 1_i64,
                "sandbox_id": failed.sandbox_id.as_str(), "day": from as i64,
                "record": {
                    "schema_version": 1_i64, "sandbox_id": failed.sandbox_id.as_str(),
                    "owner": failed.owner.as_str(), "day": from as i64,
                    "source_watermark": failed.revision as i64,
                    "range": encode(&range).unwrap(), "projection": encode(&legacy).unwrap(),
                },
            })
            .await
            .unwrap();
    }
    let range = Range {
        from: day,
        to: day + 86_400,
    };
    let daily = store
        .allocation_periods(
            &mut store.snapshot().await.unwrap(),
            std::slice::from_ref(&failed),
            range,
        )
        .await
        .unwrap();
    let projection = &daily[failed.sandbox_id.as_str()].period;
    assert_eq!(projection.uptime_seconds, 0.0);
    assert_eq!(projection.cpu_core_hours, 0.0);
    assert_eq!(projection.memory_gib_hours, 0.0);
    assert_eq!(projection.disk_gib_hours, 64.0 * 24.0);
    let cold = store
        .cold_allocation_range(
            &mut store.snapshot().await.unwrap(),
            std::slice::from_ref(&failed),
            Range {
                from: day,
                to: day + 2 * 86_400,
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cold.len(), 2);
    for rows in cold.values() {
        assert_eq!(rows[failed.sandbox_id.as_str()].period.uptime_seconds, 0.0);
        assert_eq!(
            rows[failed.sandbox_id.as_str()].period.disk_gib_hours,
            64.0 * 24.0
        );
    }
    let bucketed = store
        .bucketed_allocation_range(
            &mut store.snapshot().await.unwrap(),
            std::slice::from_ref(&failed),
            Range {
                from: day,
                to: day + 2 * 86_400,
            },
        )
        .await
        .unwrap();
    for rows in bucketed.values() {
        let period = &rows[failed.sandbox_id.as_str()].period;
        assert_eq!(period.uptime_seconds, 0.0);
        assert_eq!(period.cpu_core_hours, 0.0);
        assert_eq!(period.memory_gib_hours, 0.0);
        assert_eq!(period.disk_gib_hours, 64.0 * 24.0);
    }
    assert_eq!(
        store
            .collection("sandbox_usage_events")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    store.db.drop().await.unwrap();
}
