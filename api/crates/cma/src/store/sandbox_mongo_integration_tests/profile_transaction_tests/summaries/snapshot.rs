use super::*;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Condvar,
};
use std::time::Duration;

#[derive(Default)]
struct Pause {
    armed: AtomicBool,
    reached: tokio::sync::Notify,
    released: Mutex<bool>,
    wake: Condvar,
}

impl Pause {
    fn observe(&self, event: CommandEvent) {
        if matches!(event, CommandEvent::Started(ref event) if event.command_name == "getMore")
            && self.armed.swap(false, Ordering::SeqCst)
        {
            self.reached.notify_one();
            let (_guard, timeout) = self
                .wake
                .wait_timeout_while(
                    self.released.lock().unwrap(),
                    Duration::from_secs(10),
                    |done| !*done,
                )
                .unwrap();
            assert!(
                !timeout.timed_out(),
                "publication did not release the cursor"
            );
        }
    }

    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.wake.notify_all();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn summary_cursor_keeps_one_snapshot_during_publication() {
    let pause = Arc::new(Pause::default());
    let observed = pause.clone();
    let (client, name, store, meter) = observed_store(move |event| observed.observe(event)).await;
    // Over 16 MiB of source documents forces a real getMore; the selected
    // profile is in the suffix which has not reached the client at that point.
    seed(&store, "agent", 200, 2, true).await;
    let router = routes(store.clone());
    pause.armed.store(true, Ordering::SeqCst);
    meter.start();
    let reading = tokio::spawn({
        let router = router.clone();
        async move { get(&router, "owner", "/api/v1/agent-profiles?limit=200").await }
    });
    tokio::time::timeout(Duration::from_secs(10), pause.reached.notified())
        .await
        .unwrap();
    let publication = store
        .publish_agent_profile(
            &AgentProfileId(id("agent", 199)),
            1,
            "2026-09-06T00:00:00Z".into(),
        )
        .await;
    pause.release();
    assert_eq!(publication.unwrap().revision, 3);
    let (status, page) = reading.await.unwrap();
    let counts = meter.finish();
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["items"].as_array().unwrap().len(), 200);
    assert!(page["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|item| item["latest_revision"]["revision"] == 2));
    assert!(page["next"].is_null());
    assert!(counts.bytes > 16 * 1024 * 1024);
    let (status, fresh) = get(&router, "owner", "/api/v1/agent-profiles?limit=200").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fresh["items"][199]["latest_revision"]["revision"], 3);
    assert_eq!(
        fresh["items"][199]["latest_revision"]["published_at"],
        "2026-09-06T00:00:00Z"
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn summary_routes_refuse_aggregate_and_get_more_failures_without_partial_pages() {
    let (client, name, store, _) = measured_store().await;
    let router = routes(store.clone());
    let admin = client.database("admin");
    for kind in KINDS {
        seed(&store, kind, 1, 2, false).await;
        admin
            .run_command(doc! {
                "configureFailPoint": "failCommand", "mode": {"times": 1},
                "data": {"failCommands": ["aggregate"], "appName": &name, "errorCode": 50},
            })
            .await
            .unwrap();
        let (status, body) = get(&router, "owner", &format!("/api/v1/{kind}-profiles")).await;
        admin
            .run_command(doc! {"configureFailPoint":"failCommand", "mode":"off"})
            .await
            .unwrap();
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
        assert!(body.get("items").is_none());
        assert_eq!(
            get(&router, "owner", &format!("/api/v1/{kind}-profiles"))
                .await
                .0,
            StatusCode::OK
        );
    }
    store.agent_profiles.delete_many(doc! {}).await.unwrap();
    store.agent_revisions.delete_many(doc! {}).await.unwrap();
    seed(&store, "agent", 200, 2, true).await;
    admin
        .run_command(doc! {
            "configureFailPoint": "failCommand", "mode": {"times": 1},
            "data": {"failCommands": ["getMore"], "appName": &name, "errorCode": 50},
        })
        .await
        .unwrap();
    let (status, body) = get(&router, "owner", "/api/v1/agent-profiles?limit=200").await;
    admin
        .run_command(doc! {"configureFailPoint":"failCommand", "mode":"off"})
        .await
        .unwrap();
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert!(body.get("items").is_none());
    assert_eq!(
        get(&router, "owner", "/api/v1/agent-profiles?limit=200")
            .await
            .0,
        StatusCode::OK
    );
    client.database(&name).drop().await.unwrap();
}
