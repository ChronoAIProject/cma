use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn reconciliation_tail_recovers_once_after_repeated_slow_owner_pages() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let name = format!("cma_reconcile_owner_page_{}", uuid::Uuid::new_v4().simple());
    let app = format!("owner-page-{}", uuid::Uuid::new_v4().simple());
    let control = Client::with_uri_str(&uri).await.unwrap();
    let fixture = MongoSandboxStore::open(&control, &name).await.unwrap();
    let first = idle_connection_fixture(&fixture, "001_foreign", AgentStatus::Ready).await;
    paused_rows(&fixture, 32).await;
    let tail = idle_connection_fixture(&fixture, "900_tail", AgentStatus::Ready).await;
    let (_, left) = manager(&fixture, "left").await;
    let foreign = left.acquire(&first.id).await.unwrap().unwrap();
    left.begin_drain().await;

    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app.clone());
    let client = Client::with_options(options).unwrap();
    let reader = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (store, right) = manager(&reader, "right").await;
    let worker = configured_worker(
        store,
        Arc::new(LiveAgentRegistry::default()),
        AgentWorkerConfig::new("reconcile-owner-page", "reconcile-owner-page")
            .with_ownership(Some(right))
            .with_skip_ineligible_reconciliation(true)
            .with_defer_foreign_reconciliation(true),
    );
    let admin = control.database("admin");
    let mut sweep = ConnectionSweep::default();
    for _ in 0..2 {
        admin
            .run_command(doc! {
                "configureFailPoint": "failCommand", "mode": {"times": 1},
                "data": {
                    "appName": &app, "failCommands": ["aggregate"],
                    "blockConnection": true, "blockTimeMS": 750_i32,
                },
            })
            .await
            .unwrap();
        let started = tokio::time::Instant::now();
        let sliced = worker
            .reconcile_connection_slice(SystemTime::now(), &mut sweep)
            .await;
        admin
            .run_command(doc! {"configureFailPoint": "failCommand", "mode": "off"})
            .await
            .unwrap();
        assert!(!sliced.unwrap());
        assert!(started.elapsed() >= Duration::from_millis(200));
        assert_eq!(sweep.visited(), (1, 32));
        assert!(worker
            .reconcile_connection_slice(SystemTime::now(), &mut sweep)
            .await
            .unwrap());
    }

    let saved = fixture.get_agent(&tail.id).await.unwrap().unwrap();
    assert_eq!(saved.revision, tail.revision + 1);
    assert_eq!(saved.work.step, AgentWorkStep::PublishConnection);
    let owner = control
        .database(&name)
        .collection::<Document>(crate::store::agent_ownership::OWNERS)
        .find_one(doc! {"_id": tail.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(owner.get_i64("effect_revision").unwrap(), 1);
    assert_eq!(fixture.get_agent(&first.id).await.unwrap(), Some(first));
    left.handoff(&foreign, async { Ok(()) }).await.unwrap();
    control.database(&name).drop().await.unwrap();
}
