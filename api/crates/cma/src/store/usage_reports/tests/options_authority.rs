use super::*;
use layer2_core::{Authorization, Caller, Capability};

fn caller(capabilities: &[Capability]) -> Caller {
    Caller::new(
        SubjectId("operator".into()),
        Authorization::new(capabilities.iter().copied()),
    )
}
fn query() -> OptionQuery {
    OptionQuery {
        kind: OptionKind::Owner,
        search: "Alice".into(),
        owner: None,
        selected: None,
        cursor: None,
        limit: Some(2),
    }
}
fn assert_date_only(page: &FilterOptions) {
    assert_eq!(page.items.len(), 2);
    for item in &page.items {
        let detail = item.disambiguator.as_deref().unwrap();
        assert!(detail.starts_with("Joined 2026-11-01 · reference "));
        assert!(!detail.contains("12:30") && !detail.contains("UTC"));
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn metrics_options_reauthorize_precision_on_signed_continuations_and_fail_closed() {
    let store = fixture().await;
    let mut rows: Vec<_> = (0..4).map(|i| state(i, now() - 60)).collect();
    for (i, row) in rows.iter_mut().enumerate() {
        row.owner = SubjectId(format!("alice-{i}"));
    }
    seed(&store, &rows).await;
    store
        .collection("users")
        .insert_many((0..4).map(|i| {
            doc! {
                "_id":format!("alice-{i}"), "display_name":"Alice",
                "first_seen_at":format!("2026-11-01T12:30:00.00{}Z", i+1)
            }
        }))
        .await
        .unwrap();
    let service = MetricsService::new(store.clone());
    let sandbox_only = caller(&[Capability::ReadAllSandboxes]);
    let both = caller(&[Capability::ReadAllSandboxes, Capability::ReadUserDirectory]);
    let first = service.options(&both, query()).await.unwrap();
    assert!(first.items[0]
        .disambiguator
        .as_deref()
        .unwrap()
        .ends_with("12:30:00.001 UTC"));
    let continuation = OptionQuery {
        cursor: first.next.clone(),
        ..query()
    };
    assert!(continuation.cursor.is_some());
    // A token minted with directory access cannot retain that access after revocation.
    assert_date_only(
        &service
            .options(&sandbox_only, continuation.clone())
            .await
            .unwrap(),
    );
    let regained = service.options(&both, continuation.clone()).await.unwrap();
    assert!(regained.items[0]
        .disambiguator
        .as_deref()
        .unwrap()
        .ends_with("12:30:00.003 UTC"));
    assert_date_only(&service.options(&sandbox_only, query()).await.unwrap());
    assert!(matches!(
        service
            .options(
                &caller(&[Capability::ReadUserDirectory]),
                continuation.clone()
            )
            .await,
        Err(MetricsError::Forbidden)
    ));
    assert!(matches!(
        service.options(&caller(&[]), continuation.clone()).await,
        Err(MetricsError::Forbidden)
    ));
    let other = Caller::new(SubjectId("other".into()), both.authorization.clone());
    assert!(service.options(&other, continuation.clone()).await.is_err());
    let mut tampered = continuation;
    tampered.cursor.as_mut().unwrap().push('x');
    assert!(service.options(&both, tampered).await.is_err());
    // Missing, invalid and identical observation times must not reveal clock precision.
    for at in [None, Some("invalid"), Some("2026-11-01T12:30:00.001Z")] {
        let update = match at {
            Some(at) => doc! {"$set":{"first_seen_at":at}},
            None => doc! {"$unset":{"first_seen_at":""}},
        };
        store
            .collection("users")
            .update_many(doc! {}, update)
            .await
            .unwrap();
        let page = service.options(&sandbox_only, query()).await.unwrap();
        for item in page.items {
            let detail = item.disambiguator.unwrap();
            assert!(detail.contains("reference "));
            assert!(!detail.contains("12:30") && !detail.contains("UTC"));
        }
    }
    store.db.drop().await.unwrap();
}
