use super::*;

#[tokio::test]
async fn sandbox_availability_is_owned_validated_and_independent_of_published_content() {
    let router = fixture_with_settings(true).0;
    let mut body = sandbox_body("Warm research", None, vec![], None);
    let default = json!({"warmup_enabled": false, "warmup_count": 1, "always_keep_alive": false});
    let enabled = json!({"warmup_enabled": true, "warmup_count": 5, "always_keep_alive": true});
    body["availability"] = enabled.clone();
    let (status, error) = mutate(
        &router,
        Method::POST,
        "/api/v1/sandbox-profiles",
        Some(body.clone()),
        "owner",
        "private-warm-refused",
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(error["reason"], "warmup_requires_public_profile");
    body.as_object_mut().unwrap().remove("availability");
    let (status, profile) = mutate(
        &router,
        Method::POST,
        "/api/v1/sandbox-profiles",
        Some(body.clone()),
        "owner",
        "create-profile",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(profile["availability"], default);
    let id = profile["id"].as_str().unwrap();
    let path = format!("/api/v1/sandbox-profiles/{id}");
    let (status, pinned) = mutate(
        &router,
        Method::POST,
        &format!("{path}/revisions"),
        Some(json!({"draft_version":1})),
        "owner",
        "publish-profile",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(pinned["content"].get("availability").is_none());
    let (status, _) = mutate(
        &router,
        Method::PUT,
        &format!("{path}/visibility"),
        Some(json!({"visibility":"public"})),
        "owner",
        "share-profile",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    body["draft_version"] = json!(1);
    body["availability"] = enabled.clone();
    let (status, _) = mutate(
        &router,
        Method::PUT,
        &path,
        Some(body.clone()),
        "visitor",
        "cannot-edit-source",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, saved) = mutate(
        &router,
        Method::PUT,
        &path,
        Some(body.clone()),
        "owner",
        "enable-availability",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved["availability"], enabled);
    assert_eq!(saved["latest_revision"]["digest"], pinned["digest"]);
    // An older client saving ordinary content must preserve the live policy.
    body["draft_version"] = json!(2);
    body.as_object_mut().unwrap().remove("availability");
    let (status, preserved) = mutate(
        &router,
        Method::PUT,
        &path,
        Some(body.clone()),
        "owner",
        "legacy-save",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(preserved["availability"], enabled);
    body["draft_version"] = json!(3);
    body["availability"] = enabled.clone();
    for count in [0, 6] {
        body["availability"]["warmup_count"] = json!(count);
        let (status, error) = mutate(
            &router,
            Method::PUT,
            &path,
            Some(body.clone()),
            "owner",
            &format!("invalid-count-{count}"),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(error["reason"], "invalid_warmup_count");
    }
    let (status, fork) = mutate(
        &router,
        Method::POST,
        &format!("/api/v1/marketplace/sandbox-profiles/{id}/fork"),
        Some(json!({"revision":1})),
        "visitor",
        "customize-copy",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(fork["availability"], default);
    assert_eq!(fork["visibility"], "private");
    assert_eq!(fork["owner"], "visitor");
    assert_ne!(fork["id"], profile["id"]);
    let (status, unshared) = mutate(
        &router,
        Method::PUT,
        &format!("{path}/visibility"),
        Some(json!({"visibility":"private"})),
        "owner",
        "unshare-profile",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(unshared["availability"]["warmup_enabled"], false);
    assert_eq!(unshared["availability"]["always_keep_alive"], true);
}
