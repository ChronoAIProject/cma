use super::*;

#[tokio::test]
async fn shared_connections_use_exact_readable_endpoint_evidence_without_owner_switching() {
    for role in ["admin", "member"] {
        let (mut a, _, c) = inventory(ResourcePlatform::Lark);
        a["services"][0]["credential_source"] = json!({"type":"org","allowed":true,"role":role});
        let f = Fixture::with_data(StatusCode::OK,true,true,false,(a,json!({"endpoints":[]}),c),Some((StatusCode::OK,json!({"id":"endpoint","catalog_service_id":"catalog","auto_connected":true,"updated_at":"2026-10-27T00:00:00Z"})))).await;
        let response = f.get("?platform=lark").send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.json::<Value>().await.unwrap();
        assert_eq!(body["items"][0]["source"], "organization");
        assert_eq!(body["items"][0]["user_service_id"], "owned-connection");
        let calls = f.calls.lock().unwrap();
        assert_eq!(calls.len(), 4);
        assert!(calls
            .iter()
            .all(|(_, token)| token == "Bearer inventory-request-fixture"));
    }
}

#[tokio::test]
async fn shared_discovery_does_not_publish_denied_custom_or_mismatched_endpoint_evidence() {
    for (status, evidence) in [
        (StatusCode::FORBIDDEN, json!({})),
        (StatusCode::NOT_FOUND, json!({})),
        (
            StatusCode::OK,
            json!({"id":"other","catalog_service_id":"catalog","auto_connected":true}),
        ),
        (
            StatusCode::OK,
            json!({"id":"endpoint","catalog_service_id":"other","auto_connected":true}),
        ),
        (
            StatusCode::OK,
            json!({"id":"endpoint","catalog_service_id":"catalog","auto_connected":false}),
        ),
        (
            StatusCode::OK,
            json!({"id":"endpoint","catalog_service_id":"catalog","auto_connected":true,"url":"https://custom.example"}),
        ),
    ] {
        let (mut a, _, c) = inventory(ResourcePlatform::Lark);
        a["services"][0]["credential_source"] =
            json!({"type":"org","allowed":true,"role":"member"});
        let f = Fixture::with_data(
            StatusCode::OK,
            true,
            true,
            false,
            (a, json!({"endpoints":[]}), c),
            Some((status, evidence)),
        )
        .await;
        let response = f.get("?platform=lark").send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.json::<Value>().await.unwrap(), json!({"items":[]}));
    }
}

#[tokio::test]
async fn shared_viewers_and_wrong_region_never_trigger_endpoint_reads() {
    for (source, slug) in [
        (
            json!({"type":"org","allowed":false,"role":"viewer"}),
            "api-lark-bot",
        ),
        (
            json!({"type":"org","allowed":true,"role":"viewer"}),
            "api-lark-bot",
        ),
        (
            json!({"type":"org","allowed":true,"role":"member"}),
            "api-feishu-bot",
        ),
    ] {
        let (mut a, _, c) = inventory(ResourcePlatform::Lark);
        a["services"][0]["credential_source"] = source;
        a["services"][0]["slug"] = json!(slug);
        let f = Fixture::with_data(
            StatusCode::OK,
            true,
            true,
            false,
            (a, json!({"endpoints":[]}), c),
            None,
        )
        .await;
        assert_eq!(
            f.get("?platform=lark")
                .send()
                .await
                .unwrap()
                .json::<Value>()
                .await
                .unwrap(),
            json!({"items":[]})
        );
        assert_eq!(f.calls.lock().unwrap().len(), 3);
    }
}

#[test]
fn shared_endpoint_fanout_is_bounded() {
    let (mut a, b, c) = inventory(ResourcePlatform::Lark);
    let template = a["services"][0].clone();
    a["services"] = Value::Array(
        (0..=MAX_SHARED_ENDPOINTS)
            .map(|n| {
                let mut row = template.clone();
                row["id"] = json!(format!("connection-{n}"));
                row["endpoint_id"] = json!(format!("endpoint-{n}"));
                row["credential_source"] = json!({"type":"org","allowed":true,"role":"member"});
                row
            })
            .collect(),
    );
    assert!(shared_endpoints(
        &serde_json::from_value(a).unwrap(),
        &serde_json::from_value(b).unwrap(),
        &serde_json::from_value(c).unwrap(),
        ResourcePlatform::Lark
    )
    .is_err());
}
