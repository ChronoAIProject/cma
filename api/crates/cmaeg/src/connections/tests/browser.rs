use super::*;

#[tokio::test]
async fn browser_session_reopens_from_durable_record_and_rejects_duplicate_cookies() {
    use crate::http::connection_browser::browser_record;
    let fixture = Fixture::new().await;
    let record = connection("alice", false);
    fixture
        .store()
        .save_connection(&record, None)
        .await
        .unwrap();
    let token = fixture
        .service()
        .capability(&record, "browser", record.browser_nonce.as_deref().unwrap())
        .unwrap();
    let (reopened, version) = browser_record(&fixture.service(), &browser_headers(&token))
        .await
        .unwrap();
    assert_eq!(reopened.id, record.id);
    assert_eq!(version, 1);
    let mut duplicate_headers = browser_headers(&token);
    duplicate_headers.append(
        axum::http::header::COOKIE,
        format!("__Host-cmaeg-connection={token}").parse().unwrap(),
    );
    let mut duplicate_fields = HeaderMap::new();
    duplicate_fields.insert(
        axum::http::header::COOKIE,
        format!("__Host-cmaeg-connection={token}; __Host-cmaeg-connection={token}")
            .parse()
            .unwrap(),
    );
    for headers in [HeaderMap::new(), duplicate_headers, duplicate_fields] {
        assert!(matches!(
            browser_record(&fixture.service(), &headers).await,
            Err(Error::NotFound)
        ));
    }
}

#[tokio::test]
async fn browser_session_rejects_wrong_nonce_purpose_and_missing_record() {
    use crate::http::connection_browser::browser_record;
    let fixture = Fixture::new().await;
    let service = fixture.service();
    let record = connection("alice", false);
    fixture
        .store()
        .save_connection(&record, None)
        .await
        .unwrap();
    for (purpose, nonce) in [
        ("start", record.setup_nonce.as_str()),
        ("csrf", record.browser_nonce.as_deref().unwrap()),
        ("browser", "another-browser-private-nonce"),
    ] {
        let token = service.capability(&record, purpose, nonce).unwrap();
        assert!(matches!(
            browser_record(&service, &browser_headers(&token)).await,
            Err(Error::NotFound)
        ));
    }
    let missing = connection("alice", false);
    let token = service
        .capability(
            &missing,
            "browser",
            missing.browser_nonce.as_deref().unwrap(),
        )
        .unwrap();
    assert!(matches!(
        browser_record(&service, &browser_headers(&token)).await,
        Err(Error::NotFound)
    ));
}

#[tokio::test]
async fn cancelled_expired_or_replaced_durable_browser_grants_cannot_be_replayed() {
    use crate::http::connection_browser::browser_record;
    let fixture = Fixture::new().await;
    let service = fixture.service();
    for change in [
        "cancelled",
        "expired",
        "missing_nonce",
        "rotated_nonce",
        "changed_expiry",
        "rotated_link_nonce",
    ] {
        let mut record = connection("alice", false);
        fixture
            .store()
            .save_connection(&record, None)
            .await
            .unwrap();
        let token = service
            .capability(&record, "browser", record.browser_nonce.as_deref().unwrap())
            .unwrap();
        match change {
            "cancelled" => record.status = Status::Cancelled,
            "expired" => record.expires_at = now() - 1,
            "missing_nonce" => record.browser_nonce = None,
            "rotated_nonce" => {
                record.browser_nonce = Some("replacement-browser-private-nonce".into())
            }
            "rotated_link_nonce" => record.setup_nonce = "replacement-link-private-nonce".into(),
            _ => record.expires_at -= 30,
        }
        fixture
            .store()
            .save_connection(&record, Some(1))
            .await
            .unwrap();
        assert!(
            matches!(
                browser_record(&fixture.service(), &browser_headers(&token)).await,
                Err(Error::NotFound)
            ),
            "durable change {change} did not revoke prior browser authority"
        );
    }
}

#[tokio::test]
async fn setup_start_and_browser_capabilities_cannot_cross_operation_boundaries() {
    use crate::http::connection_browser::browser_record;
    let fixture = Fixture::new().await;
    let service = fixture.service();
    let first = connection("alice", false);
    let mut second = connection("bob", true);
    second.browser_nonce = Some("second-owner-browser-nonce".into());
    for record in [&first, &second] {
        fixture.store().save_connection(record, None).await.unwrap();
    }
    // Even a correctly authenticated capability for a different operation's
    // nonce cannot be moved onto this record or interpreted as a start grant.
    let mixed = service
        .capability(&second, "browser", first.browser_nonce.as_deref().unwrap())
        .unwrap();
    assert!(matches!(
        browser_record(&service, &browser_headers(&mixed)).await,
        Err(Error::NotFound)
    ));
    let valid = service
        .capability(&second, "browser", second.browser_nonce.as_deref().unwrap())
        .unwrap();
    let (restored, _) = browser_record(&service, &browser_headers(&valid))
        .await
        .unwrap();
    assert_eq!(restored.owner, "bob");
    assert!(restored.organization);
    assert!(matches!(
        service.open_capability(&valid, "start"),
        Err(Error::NotFound)
    ));
}

#[tokio::test]
async fn provisional_browser_reads_do_not_claim_a_link_and_first_cas_claim_excludes_other_browsers()
{
    use crate::http::connection_browser::browser_record;
    let fixture = Fixture::new().await;
    let service = fixture.service();
    let mut record = connection("alice", false);
    record.browser_nonce = None;
    record.browser_claimed = false;
    fixture
        .store()
        .save_connection(&record, None)
        .await
        .unwrap();
    let preview_token = service
        .capability(&record, "browser", "preview-browser-nonce")
        .unwrap();
    let human_token = service
        .capability(&record, "browser", "human-browser-nonce")
        .unwrap();
    let (preview, preview_version) = browser_record(&service, &browser_headers(&preview_token))
        .await
        .unwrap();
    let (mut human, human_version) = browser_record(&service, &browser_headers(&human_token))
        .await
        .unwrap();
    assert_eq!(preview_version, human_version);
    assert_ne!(preview.browser_nonce, human.browser_nonce);
    let (unchanged, version) = fixture.store().connection(&record.id).await.unwrap();
    assert_eq!(version, 1);
    assert!(!unchanged.browser_claimed);
    assert!(unchanged.browser_nonce.is_none());
    // This is the same CAS performed only after the browser POST's Origin and
    // CSRF checks. Reading or rendering a preview never reserves the nonce.
    human.browser_claimed = true;
    fixture
        .store()
        .save_connection(&human, Some(human_version))
        .await
        .unwrap();
    assert!(matches!(
        browser_record(&fixture.service(), &browser_headers(&preview_token)).await,
        Err(Error::NotFound)
    ));
    let (restored, _) = browser_record(&fixture.service(), &browser_headers(&human_token))
        .await
        .unwrap();
    assert!(restored.browser_claimed);
    assert_eq!(
        restored.browser_nonce.as_deref(),
        Some("human-browser-nonce")
    );
}

#[tokio::test]
async fn browser_http_exchange_preserves_link_preview_and_requires_origin_and_csrf_before_claim() {
    use axum::http::header;
    let fixture = Fixture::new().await;
    let service = fixture.service();
    let mut record = connection("alice", false);
    record.browser_nonce = None;
    record.browser_claimed = false;
    fixture
        .store()
        .save_connection(&record, None)
        .await
        .unwrap();
    let start_token = service
        .capability(&record, "start", &record.setup_nonce)
        .unwrap();
    let app = crate::http::connection_browser::router(service.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    struct Server(tokio::task::JoinHandle<()>);
    impl Drop for Server {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let _server = Server(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    }));
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let mut cookies = Vec::new();
    for _ in 0..2 {
        let response = client
            .get(format!("{base}/connect/start/{start_token}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers()[header::LOCATION], "/connect");
        assert_eq!(response.headers()[header::REFERRER_POLICY], "no-referrer");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
        for attribute in ["Path=/", "Secure", "HttpOnly", "SameSite=Strict"] {
            assert!(cookie.contains(attribute));
        }
        cookies.push(cookie.split(';').next().unwrap().to_owned());
    }
    assert_ne!(cookies[0], cookies[1]);
    let (unchanged, version) = fixture.store().connection(&record.id).await.unwrap();
    assert_eq!(version, 1);
    assert!(!unchanged.browser_claimed);
    assert!(unchanged.browser_nonce.is_none());
    let human_token = cookies[1].split_once('=').unwrap().1;
    let human_grant = service.open_capability(human_token, "browser").unwrap();
    let csrf = service
        .capability(&record, "csrf", &human_grant.nonce)
        .unwrap();
    let target = format!("{base}/connect/submit");
    // Invalid intent stops before any platform method while exercising real
    // HTTP extraction, Origin, CSRF and durable claim order.
    for origin in [None, Some("null"), Some("https://foreign.example")] {
        let mut request = client.post(&target).header(header::COOKIE, &cookies[1]);
        if let Some(origin) = origin {
            request = request.header(header::ORIGIN, origin);
        }
        let response = request
            .form(&[
                ("csrf_token", csrf.as_str()),
                ("intent", "fixture_invalid_intent"),
            ])
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::GONE);
    }
    let response = client
        .post(&target)
        .header(header::COOKIE, &cookies[1])
        .header(header::ORIGIN, "https://gateway.example")
        .header(header::ORIGIN, "https://gateway.example")
        .form(&[
            ("csrf_token", csrf.as_str()),
            ("intent", "fixture_invalid_intent"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::GONE);
    let response = client
        .post(&target)
        .header(header::COOKIE, &cookies[1])
        .header(header::ORIGIN, "https://gateway.example")
        .form(&[
            ("csrf_token", start_token.as_str()),
            ("intent", "fixture_invalid_intent"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::GONE);
    let (still_unclaimed, version) = fixture.store().connection(&record.id).await.unwrap();
    assert_eq!(version, 1);
    assert!(!still_unclaimed.browser_claimed);
    assert!(still_unclaimed.browser_nonce.is_none());
    let response = client
        .post(&target)
        .header(header::COOKIE, &cookies[1])
        .header(header::ORIGIN, "https://gateway.example")
        .form(&[
            ("csrf_token", csrf.as_str()),
            ("intent", "fixture_invalid_intent"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let (claimed, version) = fixture.store().connection(&record.id).await.unwrap();
    assert_eq!(version, 2);
    assert!(claimed.browser_claimed);
    assert_eq!(
        claimed.browser_nonce.as_deref(),
        Some(human_grant.nonce.as_str())
    );
    let response = client
        .get(format!("{base}/connect"))
        .header(header::COOKIE, &cookies[0])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::GONE);
    let response = client
        .get(format!("{base}/connect/start/{start_token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::GONE);
}
