//! Disabled native setup must refuse before platform or agent-provider calls.
use super::*;

#[tokio::test]
async fn disabled_lark_and_feishu_creation_has_no_external_effects() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    for region in ["lark", "feishu"] {
        for gate in ["writes", "admission", "routing"] {
            let (mut state, platform) = state(&provider, &cma, &nyxid);
            match gate {
                "writes" => state.native_lark_writes_enabled = false,
                "admission" => state.native_lark_admission_enabled = false,
                _ => state.multi_consumer_writes_enabled = false,
            }
            let (base, stop, task) = serve_app(state).await;
            let response = Client::new()
                .post(format!("{base}/v1/channels"))
                .bearer_auth("token-alice")
                .json(&body(region))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 400, "{region}/{gate}");
            let problem: Value = response.json().await.unwrap();
            assert_eq!(problem["code"], "invalid_request", "{region}/{gate}");
            assert_eq!(platform.calls.load(Ordering::SeqCst), 0);
            assert!(provider.requests().is_empty());
            assert!(cma.records().is_empty());
            stop.send(()).unwrap();
            task.await.unwrap();
        }
    }
    provider.close().await;
    cma.close().await;
    nyxid.close().await;
}
