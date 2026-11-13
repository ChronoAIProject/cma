//! Service-boundary measurements; the receiver is a fixture, not a provider.
use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn repeated_desktop_connect_windows() {
    let mode = std::env::var("CMA_DESKTOP_LATENCY_MODE").unwrap_or_else(|_| "one".into());
    assert!(["legacy", "one", "two"].contains(&mode.as_str()));
    let f = Services::new(mode != "legacy").await;
    let mut windows = Vec::new();
    for window in 0..3 {
        let mut timings = Vec::new();
        for sample in 0..105 {
            let peer = mode == "two" && sample % 2 == 1;
            let service = if peer { &f.right } else { &f.left };
            let agent = &f.agents[usize::from(peer)];
            let started = std::time::Instant::now();
            let mut connection = service
                .open_authorized(&f.caller, agent, f.authority())
                .await
                .unwrap();
            // Include a completed first frame so acquisition alone is not
            // mislabeled an operational desktop connection.
            connection
                .send(DesktopFrame::Binary(vec![1]))
                .await
                .unwrap();
            let elapsed = started.elapsed().as_secs_f64() * 1000.0;
            service.close_connection(&mut connection).await;
            if sample >= 5 {
                timings.push(elapsed);
            }
        }
        assert_eq!(timings.len(), 100);
        windows.push(serde_json::json!({"window": window, "milliseconds": timings}));
    }
    println!(
        "DESKTOP_CONNECT_MEASUREMENT={}",
        serde_json::json!({
            "mode": mode,
            "boundary": "DesktopService open_authorized through first fixture receiver frame",
            "mongo": "real disposable replica set",
            "receiver": "in-process protocol fixture",
            "credential_proof": "trusted test proof, no provider IO",
            "samples": 300,
            "errors": 0,
            "windows": windows,
        })
    );
    f.stores.db.drop().await.unwrap();
}
