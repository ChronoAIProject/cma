use super::*;
use futures_util::StreamExt;
use std::sync::atomic::AtomicBool;

#[derive(Default)]
struct Standing(AtomicBool);
#[async_trait::async_trait]
impl layer2_core::account_deletion::AccountStanding for Standing {
    async fn erasing(
        &self,
        _: &SubjectId,
    ) -> Result<bool, layer2_core::account_deletion::AccountDeletionError> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}
async fn assert_deleted(response: Response) {
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/problem+json"
    );
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), BODY_LIMIT).await.unwrap()).unwrap();
    assert_eq!(body["code"], "account_deletion_in_progress");
    assert!(body.get("items").is_none());
    assert!(body.get("sent").is_none());
}

#[tokio::test]
async fn existing_nonpersonal_custody_stays_usable_and_still_obeys_standing() {
    use base64::Engine;
    let token = format!(
        "e30.{}.fixture",
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(br#"{"sub":"owner","token_type":"access","act":{"sub":"agent"}}"#)
    );
    assert!(!crate::auth::bearer::permits_personal_authority(&token));
    for deleting in [false, true] {
        let standing = Arc::new(Standing::default());
        let fixture = Fixture::with_standing(standing.clone()).await;
        let mut state = fixture
            .store
            .subject(&SubjectId::new("owner"))
            .await
            .unwrap()
            .unwrap();
        state.tokens.as_mut().unwrap().access_token = SecretValue::new(token.clone());
        fixture.store.put_subject(state).await.unwrap();
        standing.0.store(deleting, Ordering::SeqCst);
        let response = fixture
            .send(fixture.request(Method::GET, "/v1/channels"))
            .await;
        if deleting {
            assert_deleted(response).await;
            assert_eq!(fixture.control.calls.load(Ordering::SeqCst), 0);
        } else {
            assert_eq!(response.status(), StatusCode::OK);
            let seen = fixture.control.headers.read().await;
            assert_eq!(seen[header::AUTHORIZATION], format!("Bearer {token}"));
            assert!(!seen.contains_key(header::COOKIE));
        }
    }
}

#[tokio::test]
async fn standing_portal_denies_reads_sends_and_stream_establishment() {
    let standing = Arc::new(Standing::default());
    let f = Fixture::with_standing(standing.clone()).await;
    for (method, path) in [
        (Method::GET, "/v1/channels"),
        (
            Method::POST,
            "/v1/channels/channel/outbound-messages/delivery/send",
        ),
        (Method::GET, "/v1/channels/channel/activity/stream"),
    ] {
        standing.0.store(false, Ordering::SeqCst);
        let mut request = f.request(method.clone(), path);
        request.headers_mut().insert(
            header::ORIGIN,
            HeaderValue::from_static("https://cma.example.test"),
        );
        assert_eq!(f.send(request).await.status(), StatusCode::OK);
        let before = f.control.calls.load(Ordering::SeqCst);
        standing.0.store(true, Ordering::SeqCst);
        let mut request = f.request(method, path);
        request.headers_mut().insert(
            header::ORIGIN,
            HeaderValue::from_static("https://cma.example.test"),
        );
        assert_deleted(f.send(request).await).await;
        assert_eq!(f.control.calls.load(Ordering::SeqCst), before);
    }
}

#[tokio::test]
async fn standing_portal_refuses_before_refreshing_owner_tokens() {
    let standing = Arc::new(Standing::default());
    let f = Fixture::with_standing(standing.clone()).await;
    let mut state = f
        .store
        .subject(&SubjectId::new("owner"))
        .await
        .unwrap()
        .unwrap();
    state.tokens.as_mut().unwrap().expires_at = SystemTime::now() + Duration::from_secs(10);
    f.store.put_subject(state).await.unwrap();
    standing.0.store(true, Ordering::SeqCst);
    assert_deleted(f.send(f.request(Method::GET, "/v1/channels")).await).await;
    assert_eq!(f.control.refresh_calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.control.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn standing_portal_rechecks_before_releasing_buffered_read() {
    let standing = Arc::new(Standing::default());
    let f = Fixture::with_standing(standing.clone()).await;
    let gate = Arc::new(Gate::default());
    *f.control.gate.write().await = Some(gate.clone());
    let router = f.router.clone();
    let request = f.request(Method::GET, "/v1/channels");
    let pending = tokio::spawn(async move { router.oneshot(request).await.unwrap() });
    tokio::time::timeout(Duration::from_secs(2), gate.reached.notified())
        .await
        .unwrap();
    standing.0.store(true, Ordering::SeqCst);
    gate.release.notify_one();
    assert_deleted(pending.await.unwrap()).await;
}

#[tokio::test]
async fn standing_portal_stops_open_stream_before_next_frame() {
    let standing = Arc::new(Standing::default());
    let f = Fixture::with_standing(standing.clone()).await;
    let gate = Arc::new(Gate::default());
    *f.control.gate.write().await = Some(gate.clone());
    let response = f
        .send(f.request(Method::GET, "/v1/channels/channel/activity/stream"))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();
    let first = body.next().await.unwrap().unwrap();
    assert!(String::from_utf8_lossy(&first).contains("cursor-0"));
    standing.0.store(true, Ordering::SeqCst);
    gate.release.notify_one();
    let next = tokio::time::timeout(Duration::from_secs(2), body.next())
        .await
        .unwrap();
    let error = next
        .expect("revocation must produce a typed stream error")
        .expect_err("a deleted owner's next frame must not escape");
    assert!(error.to_string().contains("account_deletion_in_progress"));
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(&error);
    let mut typed = false;
    while let Some(error) = source {
        if matches!(
            error.downcast_ref::<PortalError>(),
            Some(PortalError::AccountDeletionInProgress)
        ) {
            typed = true;
            break;
        }
        source = error.source();
    }
    assert!(
        typed,
        "the stream error must retain the typed refusal source"
    );
}
