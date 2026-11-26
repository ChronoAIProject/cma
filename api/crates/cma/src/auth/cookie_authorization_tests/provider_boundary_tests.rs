//! Adversarial provider responses must not turn warm evidence into authority.
use super::{
    fallback_tests::{browser, call},
    *,
};

async fn warm() -> (Fixture, Router, String) {
    let fixture = Fixture::new().await;
    fixture.seed().await;
    let (app, login) = browser(&fixture).await;
    assert_eq!(call(&app, &login, "/auth/session").await.0, StatusCode::OK);
    (fixture, app, login)
}

fn endpoints(fixture: &Fixture, userinfo: bool) -> (&RwLock<Reply>, &RwLock<Reply>) {
    if userinfo {
        (&fixture.control.userinfo, &fixture.control.introspection)
    } else {
        (&fixture.control.introspection, &fixture.control.userinfo)
    }
}

async fn assert_record_erased(fixture: &Fixture) {
    assert_eq!(fixture.auth.fixture_active_receipts(), 0);
}

#[tokio::test]
async fn non_200_success_and_partial_headers_never_authenticate_or_fallback() {
    for userinfo in [true, false] {
        for other_status in [200, 502, 503] {
            for variant in 0..7 {
                let (fixture, app, login) = warm().await;
                let (attack, other) = endpoints(&fixture, userinfo);
                other.write().await.status = StatusCode::from_u16(other_status).unwrap();
                let mut reply = attack.write().await;
                match variant {
                    0..=2 => reply.status = StatusCode::from_u16([202, 206, 207][variant]).unwrap(),
                    3 => {
                        reply
                            .headers
                            .insert(header::CONTENT_RANGE, "bytes 0-10/100".parse().unwrap());
                    }
                    4 => {
                        reply.headers.insert(
                            header::CONTENT_TYPE,
                            "multipart/byteranges; boundary=x".parse().unwrap(),
                        );
                    }
                    5 => {
                        reply
                            .headers
                            .insert(header::LINK, "</next>; rel=next".parse().unwrap());
                    }
                    _ => {
                        reply.status = StatusCode::SERVICE_UNAVAILABLE;
                        reply
                            .headers
                            .insert(header::CONTENT_RANGE, "bytes 0-10/100".parse().unwrap());
                    }
                }
                drop(reply);
                let before = fixture.control.requests.load(Ordering::SeqCst);
                let (status, body) = call(&app, &login, "/auth/session").await;
                assert_eq!(
                    status,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "userinfo={userinfo}, variant={variant}"
                );
                assert_eq!(body["code"], "store_failed");
                assert!(fixture.control.requests.load(Ordering::SeqCst) - before <= 2);
                assert_record_erased(&fixture).await;
            }
        }
    }
}

#[tokio::test]
async fn duplicate_dispositions_cannot_hide_provider_refusal() {
    for userinfo in [true, false] {
        for disposition in [
            r#""error":"invalid_token","error":null"#,
            r#""active":false,"active":true"#,
        ] {
            let (fixture, app, login) = warm().await;
            let (attack, other) = endpoints(&fixture, userinfo);
            attack.write().await.body =
                format!(r#"{{{disposition},"sub":"owner","permissions":["cma:*"]}}"#);
            other.write().await.status = StatusCode::SERVICE_UNAVAILABLE;
            assert_eq!(
                call(&app, &login, "/auth/session").await.0,
                StatusCode::SERVICE_UNAVAILABLE
            );
            assert_record_erased(&fixture).await;
        }
    }
}

#[tokio::test]
async fn independent_bearer_success_cannot_seed_browser_cookie_fallback() {
    let fixture = Fixture::new().await;
    fixture.seed().await;
    let (app, login) = browser(&fixture).await;
    let mut body: Value =
        serde_json::from_str(&fixture.control.introspection.read().await.body).unwrap();
    body["exp"] = json!((SystemTime::now() + Duration::from_secs(3600))
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs());
    *fixture.control.introspection.write().await = Reply::json(body);
    let token = "must-not-leak-access-token";
    assert!(identity::request_for_bearer(&fixture.auth, token.into())
        .await
        .unwrap()
        .is_some());
    assert_record_erased(&fixture).await;
    fixture.control.userinfo.write().await.status = StatusCode::SERVICE_UNAVAILABLE;
    assert_eq!(
        call(&app, &login, "/auth/session").await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[tokio::test]
async fn error_envelopes_override_claims_in_either_endpoint_and_completion_order() {
    for userinfo in [true, false] {
        for other_status in [200, 502, 503] {
            for attack_first in [true, false] {
                for (field, error, rejected) in [
                    ("error", json!("invalid_token"), true),
                    ("error", json!("revoked_token"), true),
                    ("error", json!("temporarily_unavailable"), false),
                    (
                        "error_description",
                        json!("must-not-leak-provider-details"),
                        false,
                    ),
                    ("error", json!({"code":"invalid_token"}), false),
                    ("errors", json!(["invalid_token"]), false),
                ] {
                    let (fixture, app, login) = warm().await;
                    let (attack, other) = endpoints(&fixture, userinfo);
                    let mut body: Value = serde_json::from_str(&attack.read().await.body).unwrap();
                    body[field] = error;
                    *attack.write().await = Reply::json(body);
                    other.write().await.status = StatusCode::from_u16(other_status).unwrap();
                    let delayed = if attack_first { other } else { attack };
                    delayed.write().await.delay = Duration::from_millis(5);
                    let before = fixture.control.requests.load(Ordering::SeqCst);
                    let (status, body) = call(&app, &login, "/auth/session").await;
                    assert_eq!(
                        status,
                        if rejected {
                            StatusCode::UNAUTHORIZED
                        } else {
                            StatusCode::SERVICE_UNAVAILABLE
                        }
                    );
                    assert_ne!(body["code"], "identity_unavailable");
                    assert!(fixture.control.requests.load(Ordering::SeqCst) - before <= 2);
                    assert_record_erased(&fixture).await;
                }
            }
        }
    }
}

#[tokio::test]
async fn userinfo_error_claims_without_introspection_are_also_rejected() {
    let (fixture, app, login) = warm().await;
    fixture
        .auth
        .fixture_metadata()
        .write()
        .await
        .as_mut()
        .unwrap()
        .introspection_endpoint = None;
    *fixture.control.userinfo.write().await = Reply::json(json!({
        "error":"invalid_token", "sub":"owner", "permissions":["cma:*"]
    }));
    assert_eq!(
        call(&app, &login, "/auth/session").await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_record_erased(&fixture).await;
}

#[tokio::test]
async fn started_bodies_that_stall_or_disconnect_cannot_use_warm_evidence() {
    for userinfo in [true, false] {
        for disconnect in [false, true] {
            for other_status in [200, 503] {
                let (fixture, app, login) = warm().await;
                let (attack, other) = endpoints(&fixture, userinfo);
                other.write().await.status = StatusCode::from_u16(other_status).unwrap();
                let mut reply = attack.write().await;
                reply.body = "{\"sub\":\"owner\",".into();
                reply.stream_tail = Some(if disconnect {
                    (Duration::from_millis(10), Err("truncated fixture response"))
                } else {
                    (Duration::from_secs(10), Ok("\"permissions\":[]}".into()))
                });
                drop(reply);
                let before = fixture.control.requests.load(Ordering::SeqCst);
                let (status, body) = call(&app, &login, "/auth/session").await;
                assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
                assert_eq!(body["code"], "store_failed");
                assert!(fixture.control.requests.load(Ordering::SeqCst) - before <= 2);
                assert_record_erased(&fixture).await;
            }
        }
    }
}

#[tokio::test]
async fn refusal_body_delayed_past_attempt_deadline_fails_closed() {
    for userinfo in [true, false] {
        for http_rejection in [true, false] {
            let (fixture, app, login) = warm().await;
            let (attack, other) = endpoints(&fixture, userinfo);
            other.write().await.status = StatusCode::SERVICE_UNAVAILABLE;
            let mut reply = attack.write().await;
            reply.status = if http_rejection {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::OK
            };
            reply.body = "{\"error\":".into();
            reply.stream_tail = Some((Duration::from_secs(10), Ok("\"invalid_token\"}".into())));
            drop(reply);
            let before = fixture.control.requests.load(Ordering::SeqCst);
            assert_eq!(
                call(&app, &login, "/auth/session").await.0,
                if http_rejection {
                    StatusCode::UNAUTHORIZED
                } else {
                    StatusCode::SERVICE_UNAVAILABLE
                }
            );
            assert!(fixture.control.requests.load(Ordering::SeqCst) - before <= 2);
            assert_record_erased(&fixture).await;
        }
    }
}

#[tokio::test]
async fn outer_caller_deadline_cannot_reclassify_a_started_body_as_transient() {
    let (fixture, _, _) = warm().await;
    let state = fixture
        .store
        .subject(&SubjectId::new("owner"))
        .await
        .unwrap()
        .unwrap();
    let mut reply = fixture.control.userinfo.write().await;
    reply.body = "{".into();
    reply.stream_tail = Some((Duration::from_secs(10), Ok("}".into())));
    drop(reply);
    let result = verify_with_timeout(
        &fixture.auth,
        &state.subject,
        state.clone(),
        Duration::from_millis(50),
    )
    .await;
    assert!(matches!(result, Err(StoreError::Unavailable(_))));
    assert_record_erased(&fixture).await;
}

#[tokio::test]
async fn oversized_known_and_unknown_length_bodies_cannot_use_warm_evidence() {
    for userinfo in [true, false] {
        for chunked in [true, false] {
            let (fixture, app, login) = warm().await;
            let (attack, other) = endpoints(&fixture, userinfo);
            other.write().await.status = StatusCode::SERVICE_UNAVAILABLE;
            let mut reply = attack.write().await;
            let mut body: Value = serde_json::from_str(&reply.body).unwrap();
            body["padding"] = json!("x".repeat(crate::auth::identity_http::MAX_BYTES));
            reply.body = body.to_string();
            if chunked {
                reply.stream_tail = Some((Duration::from_secs(10), Ok(String::new())));
            }
            drop(reply);
            let before = fixture.control.requests.load(Ordering::SeqCst);
            let (status, body) = call(&app, &login, "/auth/session").await;
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(body["code"], "store_failed");
            assert!(fixture.control.requests.load(Ordering::SeqCst) - before <= 2);
            assert_record_erased(&fixture).await;
        }
    }
}

#[tokio::test]
async fn oversized_stream_is_rejected_before_deserialization_or_waiting_for_its_tail() {
    use crate::auth::provider_http::{read_json_limited, ProviderReadError};
    struct MustNotDeserialize;
    impl<'de> serde::Deserialize<'de> for MustNotDeserialize {
        fn deserialize<D: serde::Deserializer<'de>>(_: D) -> Result<Self, D::Error> {
            panic!("oversized provider bytes reached JSON allocation");
        }
    }
    for chunked in [false, true] {
        let fixture = Fixture::new().await;
        let endpoint = fixture
            .auth
            .metadata()
            .await
            .unwrap()
            .userinfo_endpoint
            .unwrap();
        let mut reply = fixture.control.userinfo.write().await;
        reply.body = " ".repeat(crate::auth::identity_http::MAX_BYTES + 1);
        if chunked {
            reply.stream_tail = Some((Duration::from_secs(10), Ok("{}".into())));
        }
        drop(reply);
        let response = fixture
            .auth
            .fixture_http()
            .get(endpoint)
            .send()
            .await
            .unwrap();
        assert_eq!(response.content_length().is_none(), chunked);
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            read_json_limited::<MustNotDeserialize>(
                response,
                crate::auth::identity_http::MAX_BYTES,
            ),
        )
        .await
        .unwrap();
        assert!(matches!(result, Err(ProviderReadError::Invalid)));
    }
}

#[tokio::test]
async fn reqwest_body_timeout_is_invalid_evidence_not_retryable_transport() {
    let fixture = Fixture::new().await;
    let endpoint = fixture
        .auth
        .metadata()
        .await
        .unwrap()
        .userinfo_endpoint
        .unwrap();
    let mut reply = fixture.control.userinfo.write().await;
    reply.body = "{".into();
    reply.stream_tail = Some((Duration::from_secs(10), Ok("}".into())));
    drop(reply);
    let result = crate::auth::identity_http::json(
        fixture
            .auth
            .fixture_http()
            .get(endpoint)
            .timeout(Duration::from_millis(100)),
        tokio::time::Instant::now() + Duration::from_secs(5),
        crate::auth::verification_diagnostics::Endpoint::Userinfo,
    )
    .await;
    assert!(matches!(
        result,
        Err(ProviderVerificationError::Unavailable(_))
    ));
}

#[tokio::test]
async fn malformed_http_headers_are_not_connection_outages() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    for userinfo in [true, false] {
        let (fixture, app, login) = warm().await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/invalid", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request_prefix = [0u8; 1];
            stream.read_exact(&mut request_prefix).await.unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nbad header name: x\r\nContent-Length: 2\r\n\r\n{}")
                .await
                .unwrap();
            // Keep the socket open: the client must classify the malformed
            // response, rather than observe a connection reset from the fixture.
            tokio::time::sleep(Duration::from_secs(10)).await;
        });
        let mut metadata = fixture.auth.fixture_metadata().write().await;
        if userinfo {
            metadata.as_mut().unwrap().userinfo_endpoint = Some(endpoint);
        } else {
            metadata.as_mut().unwrap().introspection_endpoint = Some(endpoint);
        }
        drop(metadata);
        let (_, other) = endpoints(&fixture, userinfo);
        other.write().await.status = StatusCode::SERVICE_UNAVAILABLE;
        let (status, body) = call(&app, &login, "/auth/session").await;
        server.abort();
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["code"], "store_failed");
        assert_record_erased(&fixture).await;
    }
}
