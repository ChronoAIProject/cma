//! The public source operations use creator authority and flat, secret-free projections.
use super::source_support::*;
use reqwest::{Method, RequestBuilder, Response};

async fn checked(response: Response, status: u16) -> Value {
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.headers()["cmaeg-protocol-version"], "1");
    let actual = response.status();
    let value: Value = response.json().await.unwrap();
    assert_eq!(actual, status, "{value}");
    value
}
fn call(
    gateway: &Gateway,
    base: &str,
    id: &ChannelId,
    method: Method,
    suffix: &str,
) -> RequestBuilder {
    gateway
        .client
        .request(method, format!("{base}/v1/channels/{id}/sources{suffix}"))
        .bearer_auth("token-alice")
}

#[tokio::test]
async fn source_http_projection_is_flat_matches_get_and_never_returns_credentials() {
    let gateway = start().await;
    let (base, stop, task) = serve_app(gateway.state.clone()).await;
    let (id, record, _) = gateway.seed(|raw| {
        raw["routing"]["default_consumer"]["publish"]["enabled"] = true.into();
    });
    let response = call(&gateway, &base, &id, Method::POST, "").json(&json!({
        "expected_version":1,"source":routing_fixture::telegram(),
        "credentials":{"telegram_bot_token":"77:fixture-native","telegram_webhook_secret":"native-secret-fixture"}
    })).send().await.unwrap();
    let mut attached = checked(response, 201).await;
    assert_eq!(
        attached.as_object_mut().unwrap().remove("source_id"),
        Some(json!("telegram"))
    );
    assert_eq!(attached["version"], 2);
    assert_eq!(attached["record_id"], record.as_str());
    assert!(attached.get("channel").is_none());
    assert!(attached["endpoints"]["telegram_webhook_url"]
        .as_str()
        .unwrap()
        .ends_with(id.as_str()));
    let read = checked(
        gateway
            .client
            .get(format!("{base}/v1/channels/{id}"))
            .bearer_auth("token-alice")
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    assert_eq!(attached, read);
    assert_eq!(
        attached["warnings"],
        json!([{
            "pointer":"/routing/default_consumer/publish/enabled", "reason":"publish_tools_unavailable"
        }])
    );
    for secret in [
        "77:fixture-native",
        "native-secret-fixture",
        "fixture-channel-key",
        "Zml4dHVyZQ==",
    ] {
        assert!(!attached.to_string().contains(secret));
    }
    let removed = checked(
        call(
            &gateway,
            &base,
            &id,
            Method::DELETE,
            "/telegram?expected_version=2",
        )
        .send()
        .await
        .unwrap(),
        200,
    )
    .await;
    assert_eq!(removed["version"], 3);
    assert!(removed["endpoints"].get("telegram_webhook_url").is_none());
    assert!(removed.get("source_id").is_none());
    assert_eq!(gateway.calls(), 0);
    stop.send(()).unwrap();
    task.await.unwrap();
    gateway.close().await;
}

#[tokio::test]
async fn source_http_refusals_keep_owner_version_and_dynamic_flag_authority() {
    let gateway = start().await;
    let (base, stop, task) = serve_app(gateway.state.clone()).await;
    let (id, record, _) = gateway.seed(|_| {});
    let body = json!({"expected_version":1,"source":routing_fixture::telegram()});
    for method in [Method::POST, Method::DELETE] {
        let suffix = if method == Method::POST {
            ""
        } else {
            "/invalid?expected_version=1"
        };
        let url = format!("{base}/v1/channels/{id}/sources{suffix}");
        checked(
            gateway
                .client
                .request(method.clone(), &url)
                .json(&body)
                .send()
                .await
                .unwrap(),
            401,
        )
        .await;
        checked(
            gateway
                .client
                .request(method, url)
                .bearer_auth("token-bob")
                .json(&body)
                .send()
                .await
                .unwrap(),
            404,
        )
        .await;
    }
    for name in ["unknown", "agent", "custom", "relay_feishu"] {
        let problem = checked(
            call(
                &gateway,
                &base,
                &id,
                Method::DELETE,
                &format!("/{name}?expected_version=1"),
            )
            .send()
            .await
            .unwrap(),
            404,
        )
        .await;
        assert_eq!(problem["code"], "channel_not_found");
        assert_eq!(problem["detail"], "source_not_found");
    }
    checked(
        call(
            &gateway,
            &base,
            &id,
            Method::DELETE,
            "/agent?expected_version=2",
        )
        .send()
        .await
        .unwrap(),
        409,
    )
    .await;
    for suffix in [
        "/relay_lark",
        "/relay_lark?expected_version=wrong",
        "/relay_lark?expected_version=1&extra=true",
    ] {
        checked(
            call(&gateway, &base, &id, Method::DELETE, suffix)
                .send()
                .await
                .unwrap(),
            400,
        )
        .await;
    }
    for body in [
        json!({"expected_version":1,"source":routing_fixture::telegram(),"extra":true}),
        json!({"expected_version":1,"source":routing_fixture::telegram(),"credentials":{"agent_key":"unexpected"}}),
    ] {
        let problem = checked(
            call(&gateway, &base, &id, Method::POST, "")
                .json(&body)
                .send()
                .await
                .unwrap(),
            400,
        )
        .await;
        assert_eq!(problem["detail"], "request body is invalid");
    }
    assert_eq!(gateway.stored(&record).1, 1);
    stop.send(()).unwrap();
    task.await.unwrap();
    let mut disabled = gateway.state.clone();
    disabled.routing.dynamic_sources_enabled = false;
    let (base, stop, task) = serve_app(disabled).await;
    for (method, suffix) in [
        (Method::POST, ""),
        (Method::DELETE, "/relay_lark?expected_version=1"),
    ] {
        let problem = checked(
            call(&gateway, &base, &id, method, suffix)
                .json(&body)
                .send()
                .await
                .unwrap(),
            400,
        )
        .await;
        assert_eq!(problem["detail"], "dynamic_sources_disabled");
    }
    assert_eq!(gateway.stored(&record).1, 1);
    stop.send(()).unwrap();
    task.await.unwrap();
    gateway.close().await;
}

#[tokio::test]
async fn two_http_attaches_at_one_version_return_one_created_and_one_conflict() {
    let gateway = start().await;
    let (base, stop, task) = serve_app(gateway.state.clone()).await;
    let (id, record, _) = gateway.seed(|_| {});
    let body = json!({"expected_version":1,"source":routing_fixture::telegram()});
    let (left, right) = tokio::join!(
        call(&gateway, &base, &id, Method::POST, "")
            .json(&body)
            .send(),
        call(&gateway, &base, &id, Method::POST, "")
            .json(&body)
            .send()
    );
    let (left, right) = (left.unwrap(), right.unwrap());
    let mut statuses = [left.status().as_u16(), right.status().as_u16()];
    statuses.sort();
    assert_eq!(statuses, [201, 409]);
    for response in [left, right] {
        let status = response.status().as_u16();
        checked(response, status).await;
    }
    assert_eq!(gateway.stored(&record).1, 2);
    assert_eq!(gateway.index.changed.lock().unwrap().as_slice(), [2]);
    stop.send(()).unwrap();
    task.await.unwrap();
    gateway.close().await;
}
