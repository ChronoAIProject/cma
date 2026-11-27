use super::*;

#[tokio::test]
async fn authenticated_media_refusal_persists_once_without_consumers() {
    check_refusal(false, "attachment_unsupported").await;
    check_refusal(true, "content_unsupported").await;
}

async fn check_refusal(media_enabled: bool, reason: &str) {
    use std::{sync::atomic::Ordering::SeqCst, time::Duration};

    let identity = crate::fake_nyxid::FakeNyxid::start().await;
    let (owner, _) = identity.oauth.issue_tokens("user-1");
    let (other, _) = identity.oauth.issue_tokens("other-owner");
    let fixture =
        Fixture::with_ingress(|_| {}, |state| state.with_media_enabled(media_enabled)).await;
    let manager = crate::management_fixture::state(
        Default::default(),
        fixture.records.clone(),
        fixture.state.master_keys.clone(),
        &fixture.record_origin,
        &identity.base_url,
    );
    let (base, server) = serve(cmaeg::http::management_router(manager)).await;
    let url = format!("{base}/v1/channels/{}/activity", fixture.id);
    let request = |url: &str, token: &str| fixture.client.get(url).bearer_auth(token);
    let get = || async {
        let response = request(&url, &owner).send().await.unwrap();
        assert_eq!(response.status(), 200);
        response.json::<Value>().await.unwrap()
    };
    let mut photo = telegram_photo();
    caption(&mut photo, "fixture caption to omit");
    if media_enabled {
        photo.payload["content"]["text"] = "mismatched envelope caption".into();
    }
    let backend = &fixture.backend;
    let write_status = &backend.interaction_write_status;
    for status in [503, 504] {
        write_status.store(status, SeqCst);
        assert_eq!(fixture.send(&photo).await.status(), 503);
        assert_eq!(get().await["items"], json!([]));
    }
    write_status.store(200, SeqCst);
    backend.interaction_lose_response.store(true, SeqCst);
    refusal(&fixture, &photo, reason).await;
    let page = get().await;
    refusal(&fixture, &photo, reason).await;
    assert_eq!(get().await, page);
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["items"][0]["source_refusal"], reason);
    assert_eq!(page["items"][0]["consumers"], json!([]));
    assert!(!page.to_string().contains("fixture caption to omit"));
    assert!(fixture.recorder.requests.lock().unwrap().is_empty());
    let stream_url = format!("{url}/stream");
    for cursor in [None, page["cursor"].as_str()] {
        let mut get = request(&stream_url, &owner);
        if let Some(cursor) = cursor {
            get = get.header("last-event-id", cursor);
        }
        let mut response = get.send().await.unwrap().error_for_status().unwrap();
        let chunk = tokio::time::timeout(Duration::from_secs(2), response.chunk()).await;
        let frame = chunk.unwrap().unwrap().unwrap();
        let expected = format!("\"source_refusal\":\"{reason}\"");
        assert!(String::from_utf8_lossy(&frame).contains(&expected));
    }
    for url in [&url, &stream_url] {
        let response = request(url, &other).send().await.unwrap();
        assert_eq!(response.status(), 404);
    }
    server.abort();
}
