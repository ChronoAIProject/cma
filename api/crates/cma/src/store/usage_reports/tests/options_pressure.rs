use super::{allocated::request, *};
use mongodb::event::{command::CommandEvent, EventHandler};
use std::sync::atomic::{AtomicUsize, Ordering};

fn query(kind: OptionKind) -> OptionQuery {
    OptionQuery {
        kind,
        search: String::new(),
        owner: None,
        selected: None,
        cursor: None,
        limit: Some(50),
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn metrics_options_honor_circuit_without_commands_and_recover_with_polling() {
    let fixture = fixture().await;
    seed(&fixture, &[state(0, now() - 60)]).await;
    let commands = Arc::new(AtomicUsize::new(0));
    let observed = commands.clone();
    let mut config = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    config.max_pool_size = Some(4);
    // Production diagnostics deliberately clear full command observers. Install
    // this test-only counter afterward so circuit recovery is actually observed.
    super::super::budget::observe(&mut config, fixture.circuit.clone());
    config.command_event_handler = Some(EventHandler::callback(move |event| {
        if matches!(event, CommandEvent::Started(_)) {
            observed.fetch_add(1, Ordering::SeqCst);
        }
    }));
    let client = Client::with_options(config).unwrap();
    let store = Arc::new(ReportingStore {
        persistence: fixture.persistence.with_client(client),
        budgets: fixture.budgets.clone(),
        circuit: fixture.circuit.clone(),
        notifications: fixture.notifications.clone(),
        view_policy: fixture.view_policy.clone(),
    });
    let operator = scope("operator", true);
    store.circuit.store(now() + 30, Ordering::SeqCst);
    for kind in [OptionKind::Owner, OptionKind::Sandbox, OptionKind::Runtime] {
        assert!(matches!(
            store
                .options(&operator, query(kind), OwnerContextAccess::DirectoryRead)
                .await,
            Err(MetricsError::Busy)
        ));
    }
    let path = "/api/v1/admin/usage/metrics/options";
    let (status, body) = request(
        &store,
        "POST",
        path,
        "operator",
        true,
        serde_json::json!({"kind":"owner"}),
    )
    .await;
    assert_eq!(status, 429);
    assert_eq!(body["code"], "reporting_budget_exceeded");
    let (status, body) = request(&store, "GET", "/api/v1/admin/usage/metrics/reports/umr_00000000000000000000000000000001/owner-allocations?metric=cpu", "operator", true, serde_json::Value::Null).await;
    assert_eq!(status, 429);
    assert_eq!(body["code"], "reporting_budget_exceeded");
    assert_eq!(
        commands.load(Ordering::SeqCst),
        0,
        "open circuit must issue no database command"
    );
    // Move the recorded expiry into the past, without sleeping or weakening the guard.
    store.circuit.store(now() - 1, Ordering::SeqCst);
    let (status, body) = request(
        &store,
        "POST",
        path,
        "operator",
        true,
        serde_json::json!({"kind":"owner"}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    assert!(commands.load(Ordering::SeqCst) > 0);
    let report = store
        .submit(
            operator.clone(),
            ReportInput::default().validate(now()).unwrap(),
            now(),
        )
        .await
        .unwrap();
    let started = std::time::Instant::now();
    for _ in 0..4 {
        let (owners, sandboxes, runtimes, polled) =
            tokio::time::timeout(Duration::from_secs(5), async {
                tokio::try_join!(
                    store.options(
                        &operator,
                        query(OptionKind::Owner),
                        OwnerContextAccess::DirectoryRead
                    ),
                    store.options(
                        &operator,
                        query(OptionKind::Sandbox),
                        OwnerContextAccess::DirectoryRead
                    ),
                    store.options(
                        &operator,
                        query(OptionKind::Runtime),
                        OwnerContextAccess::DirectoryRead
                    ),
                    store.report(&operator, &report.report_id, now())
                )
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(owners.items.len(), 1);
        assert_eq!(sandboxes.items.len(), 1);
        assert!(runtimes.items.is_empty());
        assert_eq!(polled.report_id, report.report_id);
        assert_eq!(polled.status, ReportStatus::Pending);
    }
    assert_eq!(
        store
            .collection(JOBS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    println!(
        "option pressure: open_circuit_commands=0, concurrent_rounds=4, elapsed_ms={}",
        started.elapsed().as_millis()
    );
    fixture.db.drop().await.unwrap();
}
