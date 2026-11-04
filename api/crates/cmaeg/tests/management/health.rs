use super::*;
use std::sync::atomic::Ordering;

#[tokio::test]
async fn probe_aliases_share_readiness_without_bypassing_business_auth() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let state = management_state(&provider, &cma, &nyxid, key_ring(1));
    let readiness = state.readiness.clone();
    let (base, stop, task) = serve_app(state).await;
    let client = Client::new();

    for phase in 0..3 {
        let reasons = match phase {
            0 => vec![],
            1 => {
                readiness.routed_recovery_failed();
                vec!["routed_recovery_failed"]
            }
            _ => {
                readiness.start_draining();
                vec!["routed_recovery_failed", "draining"]
            }
        };
        for path in ["/healthz", "/v1/healthz", "/readyz", "/v1/readyz"] {
            // Invalid credentials must not turn a public probe into an auth request.
            for authorization in [None, Some("Bearer invalid-probe-credential")] {
                let mut request = client.get(format!("{base}{path}"));
                if let Some(value) = authorization {
                    request = request.header("authorization", value);
                }
                let response = request.send().await.unwrap();
                let is_readyz = path.ends_with("/readyz");
                let status = if is_readyz && phase > 0 { 503 } else { 200 };
                assert_eq!(response.status(), status, "{path}");
                assert_eq!(response.headers()["cache-control"], "no-store");
                assert_eq!(response.headers()["content-type"], "application/json");
                let expected = if is_readyz {
                    json!({"ready": phase == 0, "reasons": reasons, "cma_reachable": false})
                } else {
                    json!({"status": "ok"})
                };
                assert_eq!(
                    response.json::<serde_json::Value>().await.unwrap(),
                    expected
                );
            }
        }
    }

    // Exact aliases do not expose a namespace, or take over business handlers.
    for path in ["/v1/channels", "/v1/permissions", "/v1/channels/healthz"] {
        let response = client.get(format!("{base}{path}")).send().await.unwrap();
        assert_eq!(response.status(), 401, "{path}");
    }
    for path in ["/v1/healthz", "/v1/readyz"] {
        let response = client.post(format!("{base}{path}")).send().await.unwrap();
        assert_eq!(response.status(), 405, "{path}");
    }
    let response = client
        .get(format!("{base}/v1/healthz/extra"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
    assert!(provider.requests().is_empty());
    assert!(cma.records().is_empty());
    assert_eq!(cma.gets.load(Ordering::SeqCst), 0);
    assert_eq!(cma.lists.load(Ordering::SeqCst), 0);
    assert_eq!(nyxid.service_tokens.calls.load(Ordering::SeqCst), 0);

    // Authentication still admits an existing business route at its original path.
    let response = client
        .get(format!("{base}/v1/permissions"))
        .bearer_auth("token-alice")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);

    stop.send(()).unwrap();
    task.await.unwrap();
    provider.close().await;
    cma.close().await;
    nyxid.close().await;
}
