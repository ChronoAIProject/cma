//! Dynamic source writes and rollback-safe full channel updates.
use super::{consumer_support::Gateway, *};
use cmaeg::channels::definition::{validate_create_request, validate_update_request};
use serde_json::Value;

fn input(count: usize) -> Value {
    let mut body = create_body();
    let mut sources = Vec::new();
    for (index, platform) in ["lark", "feishu", "telegram", "discord", "slack", "whatsapp"]
        .into_iter()
        .enumerate()
    {
        sources.push(
            json!({"type":"nyxid_relay","issuer":"https://nyx.example","key_id":"route-key",
            "route_ids":[format!("5b3f0c6e-1c1d-4d39-9a8e-2f5b6c7d8e9{index}")],
            "platform":platform,"trust_normalized":matches!(platform,"discord"|"slack"|"whatsapp"),"admission":{"type":"all"}}),
        );
    }
    sources.push(json!({"type":"telegram","bot_id":"77","admission":{"type":"all"}}));
    sources.push(json!({"type":"custom","auth":{"type":"hmac","key_id":"h1"}}));
    sources.truncate(count);
    body["sources"] = sources.into();
    body["partition"] = json!({"version":1,"key":"fixed"});
    body["credentials"]["telegram_bot_token"] = "77:fixture".into();
    body["credentials"]["telegram_webhook_secret"] = "fixture-webhook".into();
    body
}
fn update(input: &Value, version: &Value) -> Value {
    let mut body = input.clone();
    body.as_object_mut().unwrap().remove("credentials");
    body["expected_version"] = version.clone();
    body
}
async fn call(request: reqwest::RequestBuilder, status: u16) -> Value {
    let response = request.bearer_auth("token-alice").send().await.unwrap();
    let actual = response.status();
    let value: Value = response.json().await.unwrap();
    assert_eq!(actual, status, "{value}");
    value
}
async fn enabled(gateway: &Gateway) -> (String, oneshot::Sender<()>, tokio::task::JoinHandle<()>) {
    let mut state = gateway.state.clone();
    state.routing.dynamic_sources_enabled = true;
    serve_app(state).await
}

#[tokio::test]
async fn full_put_accepts_a_second_shared_relay_only_with_the_write_flag() {
    let gateway = Gateway::start(false).await;
    let (on, stop, task) = enabled(&gateway).await;
    let created = call(
        gateway
            .client
            .post(format!("{}/v1/channels", gateway.base))
            .json(&input(1)),
        201,
    )
    .await;
    let id = created["channel_id"].as_str().unwrap();
    let body = update(&input(2), &created["version"]);
    let before = gateway.calls();
    let refused = call(
        gateway
            .client
            .put(format!("{}/v1/channels/{id}", gateway.base))
            .json(&body),
        400,
    )
    .await;
    assert_eq!(
        refused["detail"],
        "/sources/1: source variant is duplicated"
    );
    assert_eq!(gateway.calls(), before);
    let written = call(
        gateway
            .client
            .put(format!("{on}/v1/channels/{id}"))
            .json(&body),
        200,
    )
    .await;
    assert_eq!(
        written["definition"]["sources"].as_array().unwrap().len(),
        2
    );
    let record = written["record_id"].as_str().unwrap().parse().unwrap();
    assert_eq!(
        gateway
            .stored(&record)
            .0
            .secrets
            .channel_key
            .unwrap()
            .as_str(),
        "nyx_route_secret"
    );
    stop.send(()).unwrap();
    task.await.unwrap();
    gateway.close().await;
}

#[tokio::test]
async fn eight_sources_can_be_created_and_updated_and_carried_with_the_flag_off() {
    let gateway = Gateway::start(false).await;
    let (on, stop, task) = enabled(&gateway).await;
    let one = call(
        gateway
            .client
            .post(format!("{on}/v1/channels"))
            .json(&input(1)),
        201,
    )
    .await;
    let mut body = update(&input(8), &one["version"]);
    let id = one["channel_id"].as_str().unwrap();
    let extended = call(
        gateway
            .client
            .put(format!("{on}/v1/channels/{id}"))
            .json(&body),
        200,
    )
    .await;
    assert_eq!(
        extended["definition"]["sources"].as_array().unwrap().len(),
        8
    );
    body["expected_version"] = extended["version"].clone();
    body["name"] = "carried while flag off".into();
    let carried = call(
        gateway
            .client
            .put(format!("{}/v1/channels/{id}", gateway.base))
            .json(&body),
        200,
    )
    .await;
    assert_eq!(
        carried["definition"]["sources"],
        extended["definition"]["sources"]
    );
    body["expected_version"] = carried["version"].clone();
    body["sources"].as_array_mut().unwrap().remove(1);
    let refused = call(
        gateway
            .client
            .put(format!("{}/v1/channels/{id}", gateway.base))
            .json(&body),
        400,
    )
    .await;
    assert_eq!(
        refused["detail"],
        "/sources: must contain one to four sources"
    );
    let created = call(
        gateway
            .client
            .post(format!("{on}/v1/channels"))
            .json(&input(8)),
        201,
    )
    .await;
    assert_eq!(
        created["definition"]["sources"].as_array().unwrap().len(),
        8
    );
    let legacy = call(
        gateway
            .client
            .post(format!("{}/v1/channels", gateway.base))
            .json(&input(5)),
        400,
    )
    .await;
    assert_eq!(legacy["detail"], "/sources/4: too many sources");
    stop.send(()).unwrap();
    task.await.unwrap();
    gateway.close().await;
}

#[test]
fn public_request_validators_preserve_the_legacy_source_limit() {
    let create = serde_json::from_value(input(5)).unwrap();
    let update = serde_json::from_value(update(&input(5), &json!(1))).unwrap();
    for error in [
        validate_create_request(&create).unwrap_err(),
        validate_update_request(&update).unwrap_err(),
    ] {
        assert_eq!(error.pointer, "/sources/4");
        assert_eq!(error.message, "too many sources");
    }
}

#[tokio::test]
async fn omitted_custom_key_is_carried_only_for_the_same_partition() {
    let gateway = Gateway::start(false).await;
    let (on, stop, task) = enabled(&gateway).await;
    let mut input = input(1);
    input["sources"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"custom","auth":{"type":"hmac","key_id":"h1"}}));
    input["partition"] = json!({"version":1,"key":"conversation","custom_key":"event_subject"});
    let created = call(
        gateway
            .client
            .post(format!("{on}/v1/channels"))
            .json(&input),
        201,
    )
    .await;
    let id = created["channel_id"].as_str().unwrap();
    let mut body = update(&input, &created["version"]);
    body["partition"]
        .as_object_mut()
        .unwrap()
        .remove("custom_key");
    let retained = call(
        gateway
            .client
            .put(format!("{}/v1/channels/{id}", gateway.base))
            .json(&body),
        200,
    )
    .await;
    assert_eq!(
        retained["definition"]["partition"]["custom_key"],
        "event_subject"
    );
    body["expected_version"] = retained["version"].clone();
    for (partition, detail) in [
        (
            json!({"version":1,"key":"conversation_and_sender"}),
            "/partition/key: custom sources require fixed, event_subject, or per_event",
        ),
        (
            json!({"version":2,"key":"conversation"}),
            "/partition/version: unsupported version",
        ),
        (
            json!({"version":1,"key":"fixed","custom_key":"event_subject"}),
            "/partition/custom_key: requires a chat partition key",
        ),
        (
            json!({"version":1,"key":"conversation","custom_key":"thread"}),
            "/partition/custom_key: must be fixed, event_subject, or per_event",
        ),
    ] {
        body["partition"] = partition;
        let refused = call(
            gateway
                .client
                .put(format!("{on}/v1/channels/{id}"))
                .json(&body),
            400,
        )
        .await;
        assert_eq!(refused["detail"], detail);
    }
    body["partition"] = json!({"version":1,"key":"fixed"});
    let changed = call(
        gateway
            .client
            .put(format!("{on}/v1/channels/{id}"))
            .json(&body),
        200,
    )
    .await;
    assert!(changed["definition"]["partition"]
        .get("custom_key")
        .is_none());
    stop.send(()).unwrap();
    task.await.unwrap();
    gateway.close().await;
}
