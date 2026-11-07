use super::*;
use layer2_core::sandbox_metrics::OwnerContextAccess;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn metrics_owner_labels_distinguish_page_boundaries_with_authorized_context() {
    let store = fixture().await;
    let at = now();
    let report = store
        .submit(
            scope("operator", true),
            ReportInput::default().validate(at).unwrap(),
            at,
        )
        .await
        .unwrap();
    let lease = store
        .claim_report("report:0", "boundary")
        .await
        .unwrap()
        .unwrap();
    let states: Vec<_> = (0..201)
        .map(|n| {
            let mut state = state(n, at - 3600);
            state.owner = SubjectId(format!("owner-{n:06}"));
            state
        })
        .collect();
    seed(&store, &states).await;
    let rows: Vec<_> = states
        .iter()
        .map(|state| {
            let mut row = fixture_row(state);
            row.allocated_period.cpu_core_hours = 1.;
            row
        })
        .collect();
    store
        .write_report_chunk(&lease, &rows, vec![])
        .await
        .unwrap();
    store
        .finish_report(&lease, report.clone(), true)
        .await
        .unwrap();
    store
        .collection("users")
        .insert_many((0..201).map(|n| {
            doc! {
                "_id":format!("owner-{n:06}"),
                "display_name":if n >= 199 {"Alex Chen".into()} else {format!("Owner {n}")}
            }
        }))
        .await
        .unwrap();

    for (timestamps, access, reference) in [
        (
            ["2026-01-01T00:00:00Z", "2026-02-01T00:00:00Z"],
            OwnerContextAccess::DateOnly,
            false,
        ),
        (
            ["2026-01-01T10:00:00Z", "2026-01-01T11:00:00Z"],
            OwnerContextAccess::DirectoryRead,
            false,
        ),
        (
            ["2026-01-01T10:00:00Z", "2026-01-01T11:00:00Z"],
            OwnerContextAccess::DateOnly,
            true,
        ),
        (
            ["2026-01-01T10:00:00Z", "2026-01-01T10:00:00Z"],
            OwnerContextAccess::DirectoryRead,
            true,
        ),
        (["invalid", "invalid"], OwnerContextAccess::DateOnly, true),
    ] {
        for (index, timestamp) in timestamps.iter().enumerate() {
            store
                .collection("users")
                .update_one(
                    doc! {"_id":format!("owner-{:06}",199+index)},
                    doc! {"$set":{"first_seen_at":timestamp}},
                )
                .await
                .unwrap();
        }
        let query = |limit, cursor| OwnerAllocationQuery {
            metric: Metric::Cpu,
            limit: Some(limit),
            cursor,
        };
        let first = store
            .owner_allocations(&report, query(200, None), access)
            .await
            .unwrap();
        let second = store
            .owner_allocations(&report, query(200, first.next.clone()), access)
            .await
            .unwrap();
        assert!(first.items[..199]
            .iter()
            .all(|item| item.disambiguator.is_none()));
        assert!(second.next.is_none());
        let pair = [&first.items[199], &second.items[0]];
        assert_eq!(pair[0].label, "Alex Chen");
        assert_eq!(pair[1].label, "Alex Chen");
        assert_ne!(pair[0].disambiguator, pair[1].disambiguator);
        for item in pair {
            let detail = item.disambiguator.as_ref().unwrap();
            assert_eq!(detail.contains("reference"), reference, "{detail}");
            assert!(!detail.contains(&item.subject));
            if matches!(access, OwnerContextAccess::DateOnly) {
                assert!(!detail.contains("UTC"));
            }
        }
        // Identity is independent of page size and of whether a peer is on this page.
        let tiny = store
            .owner_allocations(&report, query(1, first.next), access)
            .await
            .unwrap();
        assert_eq!(tiny.items[0].disambiguator, pair[1].disambiguator);
    }
    store.db.drop().await.unwrap();
}
