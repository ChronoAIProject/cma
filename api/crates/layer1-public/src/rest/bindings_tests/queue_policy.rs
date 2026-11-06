use super::*;

#[tokio::test]
async fn queue_lifetime_defaults_and_limits_are_enforced_before_binding_creation() {
    for (seconds, status, effective) in [
        (None, StatusCode::OK, 300),
        (Some(1), StatusCode::OK, 1),
        (Some(0), StatusCode::BAD_REQUEST, 0),
        (Some(86401), StatusCode::BAD_REQUEST, 0),
    ] {
        let (app, store) = app();
        let mut input = body();
        if let Some(seconds) = seconds {
            input["profile"]["queue_lifetime_seconds"] = json!(seconds);
        }
        input["profile_digest"] = json!(format!(
            "sha256:{}",
            content_digest(&input["profile"]).unwrap()
        ));
        let response = app
            .oneshot(request(
                "PUT",
                "/api/v1/cmaeg/bindings/queue-limits",
                Some(input),
                Some(authority()),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        if status == StatusCode::OK {
            assert_eq!(
                json_body(response).await["queue_lifetime_seconds"],
                effective
            );
        } else {
            assert!(store.binding("queue-limits").await.unwrap().is_none());
        }
    }
}

#[tokio::test]
async fn binding_owner_can_opt_into_a_bounded_queue_lifetime_and_read_it_back() {
    let (app, _) = app();
    let mut input = body();
    input["profile"]["queue_lifetime_seconds"] = json!(86400);
    input["profile_digest"] = json!(format!(
        "sha256:{}",
        content_digest(&input["profile"]).unwrap()
    ));
    let path = "/api/v1/cmaeg/bindings/long-queue";
    let response = app
        .clone()
        .oneshot(request("PUT", path, Some(input), Some(authority())))
        .await
        .unwrap();
    let status = response.status();
    let value = json_body(response).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    assert_eq!(value["queue_lifetime_seconds"], 86400);
    assert!(value["honoured"]
        .as_array()
        .unwrap()
        .contains(&json!("queue_lifetime_seconds")));
    let read = app
        .oneshot(request("GET", path, None, Some(authority())))
        .await
        .unwrap();
    assert_eq!(read.status(), StatusCode::OK);
    assert_eq!(json_body(read).await["queue_lifetime_seconds"], 86400);
}
