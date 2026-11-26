use super::*;

async fn joined_fixture() -> (Fixture, HeaderMap) {
    let mut fixture = Fixture::new().await;
    Arc::get_mut(&mut fixture.auth)
        .unwrap()
        .enable_fixture_browser_session_join();
    fixture.seed().await;
    fixture.auth.metadata().await.unwrap();
    let cookie = fixture
        .auth
        .issue_browser_login(SubjectId::new("owner"))
        .await
        .unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(
        header::COOKIE,
        format!("cma_session={cookie}").parse().unwrap(),
    );
    (fixture, headers)
}

#[tokio::test]
async fn joined_session_still_verifies_every_request_and_rejects_provider_revocation() {
    let (fixture, headers) = joined_fixture().await;
    fixture.control.requests.store(0, Ordering::SeqCst);
    for _ in 0..2 {
        assert!(fixture
            .auth
            .resolve_request(&headers)
            .await
            .unwrap()
            .is_some());
    }
    assert_eq!(fixture.control.requests.load(Ordering::SeqCst), 4);
    *fixture.control.introspection.write().await = Reply::json(json!({"active":false}));
    assert!(fixture
        .auth
        .resolve_request(&headers)
        .await
        .unwrap()
        .is_none());
    assert_eq!(fixture.control.requests.load(Ordering::SeqCst), 6);
}

#[tokio::test]
async fn joined_session_rechecks_logout_after_provider_verification() {
    let (fixture, headers) = joined_fixture().await;
    let gate = Arc::new(Gate::default());
    fixture.control.introspection.write().await.gate = Some(gate.clone());
    let auth = fixture.auth.clone();
    let pending_headers = headers.clone();
    let pending = tokio::spawn(async move { auth.resolve_request(&pending_headers).await });
    tokio::time::timeout(Duration::from_secs(2), gate.reached.notified())
        .await
        .unwrap();
    let cookie =
        crate::auth::session_routes::request_cookie(&headers, crate::auth::AUTH_COOKIE).unwrap();
    let digest = SecretDigest::identify(crate::auth::AUTH_COOKIE_DOMAIN, cookie);
    fixture.store.delete_browser_login(&digest).await.unwrap();
    gate.release.notify_one();
    assert!(pending.await.unwrap().unwrap().is_none());
}

#[tokio::test]
async fn joined_session_rechecks_token_generation_after_provider_verification() {
    let (fixture, headers) = joined_fixture().await;
    let gate = Arc::new(Gate::default());
    fixture.control.introspection.write().await.gate = Some(gate.clone());
    let auth = fixture.auth.clone();
    let pending = tokio::spawn(async move { auth.resolve_request(&headers).await });
    tokio::time::timeout(Duration::from_secs(2), gate.reached.notified())
        .await
        .unwrap();
    let mut replacement = fixture
        .store
        .subject(&SubjectId::new("owner"))
        .await
        .unwrap()
        .unwrap();
    replacement.revision += 1;
    replacement.token_generation += 1;
    replacement.tokens.as_mut().unwrap().access_token =
        SecretValue::new("rotated-fixture-access".into());
    fixture.store.put_subject(replacement).await.unwrap();
    gate.release.notify_one();
    assert!(matches!(
        pending.await.unwrap(),
        Err(StoreError::Unavailable(_))
    ));
}

#[tokio::test]
async fn verification_counts_led_and_shared_without_sensitive_fields() {
    use tracing::instrument::WithSubscriber;
    #[derive(Clone)]
    struct Writer(Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let bytes = Arc::new(std::sync::Mutex::new(Vec::new()));
    let writer = Writer(bytes.clone());
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_max_level(tracing::Level::DEBUG)
        .with_writer(move || writer.clone())
        .finish();
    async {
        let fixture = Fixture::new().await;
        let state = fixture.seed().await;
        fixture.auth.metadata().await.unwrap();
        let gate = Arc::new(Gate::default());
        fixture.control.introspection.write().await.gate = Some(gate.clone());
        let (a, b, ()) = tokio::join!(
            request_for_browser_cookie(&fixture.auth, &state.subject, state.clone()),
            request_for_browser_cookie(&fixture.auth, &state.subject, state.clone()),
            async {
                gate.reached.notified().await;
                gate.release.notify_one();
            }
        );
        assert!(a.unwrap().is_some());
        assert!(b.unwrap().is_some());
    }
    .with_subscriber(subscriber)
    .await;
    let text = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
    let lines: Vec<_> = text
        .lines()
        .filter(|line| line.contains("cookie_identity_resolution"))
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(
        lines
            .iter()
            .filter(|line| line.contains("source=\"led\""))
            .count(),
        1
    );
    assert_eq!(
        lines
            .iter()
            .filter(|line| line.contains("source=\"shared\""))
            .count(),
        1
    );
    for line in lines {
        for private in [
            "fixture",
            "owner",
            "token",
            "subject",
            "cma_session",
            "http:",
        ] {
            assert!(!line.contains(private), "{line}");
        }
    }
}
