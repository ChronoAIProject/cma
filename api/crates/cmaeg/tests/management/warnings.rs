//! Warning projection is shared by reads, recovery and mutation responses.
use super::{consumer_support::*, *};
use serde_json::Value;

async fn response(request: reqwest::RequestBuilder, status: u16) -> Value {
    let response = request.bearer_auth("token-alice").send().await.unwrap();
    assert_eq!(response.status(), status);
    assert_eq!(response.headers()["cache-control"], "no-store");
    response.json().await.unwrap()
}

#[tokio::test]
async fn warnings_follow_mutations_reads_and_read_only_recovery_then_disappear() {
    let gateway = Gateway::start(true).await;
    let mut input = create_body();
    input["record_id"] = uuid::Uuid::new_v4().to_string().into();
    let created = response(
        gateway
            .client
            .post(format!("{}/v1/channels", gateway.base))
            .json(&input),
        201,
    )
    .await;
    assert!(created.get("warnings").is_none());
    let id = created["channel_id"].as_str().unwrap();
    let record = created["record_id"].as_str().unwrap();
    let url = format!("{}/v1/channels/{id}", gateway.base);
    let routed = response(gateway.client.put(format!("{url}/routing")).json(&json!({
        "expected_version":created["version"], "default_consumer":{"name":"main","publish":{"enabled":true,"targets":[]}},
        "default_targets":["default"]
    })), 200).await;
    let expected = json!([{"pointer":"/routing/default_consumer/publish/enabled","reason":"publish_tools_unavailable"}]);
    assert_eq!(routed["warnings"], expected);
    let calls = gateway.calls();
    for read_url in [
        url.clone(),
        format!("{}/v1/channel-records/{record}", gateway.base),
    ] {
        let read = response(gateway.client.get(read_url), 200).await;
        assert_eq!(read["warnings"], expected);
        assert_eq!(read["channel_id"], created["channel_id"]);
        assert_eq!(read["version"], routed["version"]);
    }
    assert_eq!(gateway.calls(), calls);
    let updated = response(
        gateway
            .client
            .put(format!("{url}/consumers/default"))
            .json(&json!({
                "expected_version":routed["version"], "name":"main", "label":"Main"
            })),
        200,
    )
    .await;
    assert_eq!(updated["channel"]["warnings"], expected);
    let cleared = response(gateway.client.put(format!("{url}/consumers/default")).json(&json!({
        "expected_version":updated["channel"]["version"],"name":"main","publish":{"enabled":false,"targets":[]}
    })), 200).await;
    assert!(cleared["channel"].get("warnings").is_none());
    let exited = response(
        gateway.client.delete(format!(
            "{url}/routing?expected_version={}",
            cleared["channel"]["version"]
        )),
        200,
    )
    .await;
    assert!(exited.get("warnings").is_none());
    gateway.close().await;
}

#[tokio::test]
async fn routed_reads_warn_while_the_switch_is_off_using_configured_capabilities() {
    let gateway = Gateway::start(false).await;
    let (id, _, _) = gateway.seed(|raw| {
        raw["sources"]
            .as_array_mut()
            .unwrap()
            .push(routing_fixture::custom());
        raw["partition"]["key"] = "fixed".into();
        raw["routing"]["default_consumer"]["publish"] = json!({"enabled":true,"targets":[]});
    });
    let mut state = gateway.state.clone();
    state.media_tool_providers = ["cma_codex".parse().unwrap()].into_iter().collect();
    state.routing.non_human_providers.insert("cma_codex".into());
    let (configured, stop, task) = serve_app(state).await;
    let warned = response(
        gateway
            .client
            .get(format!("{}/v1/channels/{id}", gateway.base)),
        200,
    )
    .await;
    assert_eq!(
        warned["warnings"],
        json!([
            {"pointer":"/routing/default_targets/0","reason":"target_ignores_custom_events"},
            {"pointer":"/routing/default_consumer/publish/enabled","reason":"publish_tools_unavailable"}
        ])
    );
    let supported = response(
        gateway.client.get(format!("{configured}/v1/channels/{id}")),
        200,
    )
    .await;
    assert!(supported.get("warnings").is_none());
    stop.send(()).unwrap();
    task.await.unwrap();
    gateway.close().await;
}
