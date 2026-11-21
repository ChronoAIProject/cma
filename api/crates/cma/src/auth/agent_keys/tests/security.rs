use super::*;

#[tokio::test]
async fn partial_agent_key_responses_cannot_establish_service_authority() {
    let fixture = Fixture::new().await;
    let auth = fixture.auth().await;
    let key = "nyx_fixture-security-boundary";
    assert!(auth.resolve_request(&headers(key)).await.unwrap().is_some());
    for status in [202, 206, 207] {
        *fixture.status.write().await = StatusCode::from_u16(status).unwrap();
        let before = fixture.introspections.load(Ordering::SeqCst);
        assert!(auth.resolve_request(&headers(key)).await.is_err());
        assert_eq!(fixture.introspections.load(Ordering::SeqCst) - before, 1);
    }
    *fixture.status.write().await = StatusCode::OK;
    for (name, value) in [
        (header::CONTENT_RANGE, "bytes 0-10/100"),
        (header::CONTENT_TYPE, "multipart/byteranges; boundary=x"),
        (header::LINK, "</next>; rel=next"),
    ] {
        fixture.response_headers.write().await.clear();
        fixture
            .response_headers
            .write()
            .await
            .insert(name, value.parse().unwrap());
        assert!(auth.resolve_request(&headers(key)).await.is_err());
    }
}

#[tokio::test]
async fn error_envelopes_and_oversized_agent_key_claims_never_establish_authority() {
    let fixture = Fixture::new().await;
    let auth = fixture.auth().await;
    let key = "nyx_fixture-security-boundary";
    let original = fixture.response.read().await.clone();
    for (field, value, rejected) in [
        ("error", json!("invalid_token"), true),
        ("error", json!("revoked_token"), true),
        ("error", json!("temporarily_unavailable"), false),
        (
            "error_description",
            json!("must-not-leak-provider-details"),
            false,
        ),
        (
            "padding",
            json!("x".repeat(crate::auth::identity_http::MAX_BYTES)),
            false,
        ),
    ] {
        let mut body = original.clone();
        body[field] = value;
        *fixture.response.write().await = body;
        let before = fixture.introspections.load(Ordering::SeqCst);
        let result = auth.resolve_request(&headers(key)).await;
        if rejected {
            assert!(matches!(result, Ok(None)));
        } else {
            assert!(result.is_err());
        }
        assert_eq!(fixture.introspections.load(Ordering::SeqCst) - before, 1);
    }
}
