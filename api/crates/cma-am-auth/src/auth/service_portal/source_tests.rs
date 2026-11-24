use super::*;

#[tokio::test]
async fn source_test_forwards_exact_diagnostic_post_for_discord_and_relay() {
    let f = Fixture::new().await;
    *f.control.permissions.write().await = vec!["cma-eg:channels:test".into()];
    let mut expected = Vec::new();
    for (channel, platform) in [("saved-discord", "discord"), ("saved-relay", "telegram")] {
        let path = format!("/v1/channels/{channel}/sources/0/test");
        let mut request = f.post(&path);
        for name in ["x-cmaeg-owner", "x-forwarded-user", "x-forwarded-for"] {
            request
                .headers_mut()
                .insert(name, HeaderValue::from_static("other-owner"));
        }
        let response = f.send(request).await;
        assert_eq!(response.status(), StatusCode::OK, "{platform}");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let body = to_bytes(response.into_body(), BODY_LIMIT).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap(),
            source_check_response(platform)
        );
        let headers = f.control.headers.read().await;
        assert_eq!(headers[header::AUTHORIZATION], "Bearer initial-test-access");
        for name in [
            "cookie",
            "x-cmaeg-owner",
            "x-forwarded-user",
            "x-forwarded-for",
        ] {
            assert!(!headers.contains_key(name), "{name}");
        }
        expected.push((Method::POST, path));
        assert_eq!(*f.control.requests.read().await, expected);
    }
}

#[tokio::test]
async fn source_test_preserves_gateway_owner_permission_audience_and_index_refusals() {
    let f = Fixture::new().await;
    *f.control.permissions.write().await = vec!["cma-eg:channels:test".into()];
    let max_index = usize::MAX.to_string();
    for (channel, index) in [
        ("foreign", "0"),
        ("saved-discord", "1"),
        ("saved-relay", max_index.as_str()),
    ] {
        let path = format!("/v1/channels/{channel}/sources/{index}/test");
        let mut request = f.post(&path);
        request
            .headers_mut()
            .insert("x-cmaeg-owner", HeaderValue::from_static("other-owner"));
        let response = f.send(request).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = to_bytes(response.into_body(), BODY_LIMIT).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap(),
            json!({"status":404,"detail":"Source not found."})
        );
        assert_eq!(
            f.control.requests.read().await.last(),
            Some(&(Method::POST, path))
        );
        assert!(!f.control.headers.read().await.contains_key("x-cmaeg-owner"));
    }
    *f.control.permissions.write().await = vec!["cma-eg:channels:read".into()];
    let path = "/v1/channels/saved-discord/sources/0/test";
    assert_eq!(f.send(f.post(path)).await.status(), StatusCode::FORBIDDEN);
    *f.control.permissions.write().await = vec!["cma-eg:channels:test".into()];
    *f.control.resource.write().await = Some("https://wrong.example/resource".into());
    assert_eq!(
        f.send(f.post(path)).await.status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn source_test_requires_browser_custody_and_exact_origin_before_dispatch() {
    let f = Fixture::new().await;
    let path = "/v1/channels/saved-discord/sources/0/test";
    for origin in [
        None,
        Some("null"),
        Some("https://other.example"),
        Some("https://cma.example.test.evil.example"),
    ] {
        let mut request = f.request(Method::POST, path);
        if let Some(origin) = origin {
            request
                .headers_mut()
                .insert(header::ORIGIN, origin.parse().unwrap());
        }
        assert_eq!(f.send(request).await.status(), StatusCode::FORBIDDEN);
    }
    let mut anonymous = f.post(path);
    anonymous.headers_mut().remove(header::COOKIE);
    assert_eq!(f.send(anonymous).await.status(), StatusCode::UNAUTHORIZED);
    let mut bearer = f.post(path);
    bearer.headers_mut().insert(
        header::AUTHORIZATION,
        HeaderValue::from_static("Bearer other-access"),
    );
    assert_eq!(f.send(bearer).await.status(), StatusCode::UNAUTHORIZED);
    let mut duplicate = f.post(path);
    duplicate.headers_mut().append(
        header::COOKIE,
        HeaderValue::from_static("cma_session=another"),
    );
    assert_eq!(f.send(duplicate).await.status(), StatusCode::UNAUTHORIZED);
    f.auth.revoke_browser_login(&f.cookie).await.unwrap();
    assert_eq!(
        f.send(f.post(path)).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert!(f.control.requests.read().await.is_empty());
}

#[tokio::test]
async fn source_test_rejects_noncanonical_paths_and_other_methods_before_dispatch() {
    let f = Fixture::new().await;
    for index in [
        "",
        "+1",
        "-1",
        "00",
        "01",
        "1.0",
        "1e0",
        "0x1",
        "18446744073709551616",
        "9999999999999999999999999999999999999999",
        "%30",
        "%2F",
        "0%2f1",
        "%2e%2e",
        ".",
        "..",
        "0/..",
        "0/1",
    ] {
        let path = format!("/v1/channels/saved-discord/sources/{index}/test");
        assert_eq!(
            f.send(f.post(&path)).await.status(),
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
    for channel in ["", ".", "..", "%2e%2e", "a%2Fb", "a%252Fb", "a/b", "a/../b"] {
        let path = format!("/v1/channels/{channel}/sources/0/test");
        assert_eq!(
            f.send(f.post(&path)).await.status(),
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
    let oversized_id = "a".repeat(8193);
    assert_eq!(
        f.send(f.post(&format!("/v1/channels/{oversized_id}/sources/0/test")))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    for path in [
        "/v1/channels/saved-discord/sources/0/test/",
        "/v1/channels/saved-discord/sources/0/test/send",
        "/v1/channels/saved-discord/sources/0",
        "/v1/channels/saved-discord/sources/0/rekey",
        "/v1/channels/saved-discord/consumers/0/test",
    ] {
        assert_eq!(
            f.send(f.post(path)).await.status(),
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
    for method in [
        Method::GET,
        Method::HEAD,
        Method::PUT,
        Method::PATCH,
        Method::DELETE,
        Method::OPTIONS,
        Method::TRACE,
    ] {
        let mut request = f.post("/v1/channels/saved-discord/sources/0/test");
        *request.method_mut() = method.clone();
        assert_eq!(
            f.send(request).await.status(),
            StatusCode::NOT_FOUND,
            "{method}"
        );
    }
    assert!(f.control.requests.read().await.is_empty());
}
