use super::*;
use crate::config::report_views::{ViewPolicy, WarmScope};
use crate::store::migrations::usage_projections::{self, COLLECTION};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn projected_reports_preserve_series_owner_pages_and_fail_closed() {
    let old = fixture().await;
    let store = old.with_view_policy(ViewPolicy {
        aggregate_projections_enabled: true,
        view_scopes: vec![WarmScope {
            admin: true,
            principal: "operator".into(),
            sandbox_id: None,
        }],
        ..Default::default()
    });
    old.require_projection_schema().await.unwrap();
    assert!(store.require_projection_schema().await.is_err());
    for (collection, index) in usage_projections::inspect(&store.db).await.unwrap() {
        store
            .db
            .run_command(doc! {"createIndexes":collection,"indexes":[index]})
            .await
            .unwrap();
    }
    store.require_projection_schema().await.unwrap();

    let at = now();
    let states: Vec<_> = (0..24)
        .map(|id| {
            let mut row = state(id, at - 3600);
            row.owner = SubjectId(format!("owner-{:02}", id % 22));
            row
        })
        .collect();
    seed(&store, &states).await;
    store.collection("users").insert_many([
        doc! {"_id":"owner-00","display_name":"Fixture Owner","first_seen_at":"2026-01-01T00:00:00Z"},
        doc! {"_id":"owner-01","display_name":"Fixture Owner","first_seen_at":"2026-02-01T00:00:00Z"},
    ]).await.unwrap();
    let owner = scope("operator", true);
    let pending = store
        .submit(
            owner.clone(),
            ReportInput::default().validate(at).unwrap(),
            at,
        )
        .await
        .unwrap();
    let lease = store
        .claim_report("report:0", "aggregate-fixture")
        .await
        .unwrap()
        .unwrap();
    let generated =
        crate::sandbox_metrics::generate_fixture_with_collection(store.clone(), &lease, false)
            .await
            .unwrap();
    assert_ne!(generated.summary.as_ref().unwrap().sandbox_count, 0);
    store.finish_report(&lease, generated, true).await.unwrap();
    let report = store
        .report(&owner, &pending.report_id, now())
        .await
        .unwrap();
    let generation = store.projected_generation(&report).await.unwrap().unwrap();
    assert_eq!(generation, lease.generation);

    for metric in [Metric::Cpu, Metric::Memory, Metric::Disk] {
        let total = SeriesQuery {
            metric,
            group_by: GroupBy::Total,
            series_id: None,
            top: None,
        };
        let cases = std::iter::once(total.clone())
            .chain(
                [GroupBy::Owner, GroupBy::Sandbox]
                    .into_iter()
                    .flat_map(|group_by| {
                        (1..=20).map(move |top| SeriesQuery {
                            metric,
                            group_by,
                            series_id: None,
                            top: Some(top),
                        })
                    }),
            )
            .chain([
                SeriesQuery {
                    group_by: GroupBy::Owner,
                    series_id: Some("owner-21".into()),
                    ..total.clone()
                },
                SeriesQuery {
                    group_by: GroupBy::Sandbox,
                    series_id: Some(states[23].sandbox_id.to_string()),
                    ..total
                },
            ]);
        for query in cases {
            let legacy = old.series(&report, query.clone()).await.unwrap();
            let cached = store.series(&report, query).await.unwrap();
            assert_eq!(
                serde_json::to_value(cached).unwrap(),
                serde_json::to_value(legacy).unwrap()
            );
        }
        let first = OwnerAllocationQuery {
            metric,
            limit: Some(1),
            cursor: None,
        };
        let next = old
            .owner_allocations(&report, first.clone(), OwnerContextAccess::DateOnly)
            .await
            .unwrap()
            .next;
        for query in [
            first,
            OwnerAllocationQuery {
                metric,
                limit: Some(1),
                cursor: next,
            },
        ] {
            let legacy = old
                .owner_allocations(&report, query.clone(), OwnerContextAccess::DateOnly)
                .await
                .unwrap();
            let cached = store
                .owner_allocations(&report, query, OwnerContextAccess::DateOnly)
                .await
                .unwrap();
            assert_eq!(
                serde_json::to_value(cached.items).unwrap(),
                serde_json::to_value(legacy.items).unwrap()
            );
            assert_eq!(cached.total_count, legacy.total_count);
        }
    }

    let removed = store
        .collection(COLLECTION)
        .delete_many(doc! {
            "report_id":&report.report_id,"kind":"chart_total","metric":Metric::Cpu.index() as i64,
        })
        .await
        .unwrap();
    assert!(removed.deleted_count > 0);
    let total = SeriesQuery {
        metric: Metric::Cpu,
        group_by: GroupBy::Total,
        series_id: None,
        top: None,
    };
    assert!(matches!(
        store.series(&report, total.clone()).await,
        Err(MetricsError::Unavailable)
    ));
    assert!(old.series(&report, total).await.is_ok());
    store.db.drop().await.unwrap();
}
