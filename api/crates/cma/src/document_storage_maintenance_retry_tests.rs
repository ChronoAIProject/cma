use super::{failure_tests, maintenance_tests::*, *};
use crate::dependency_test_support::{finish, Peer};
use layer2_core::document_artifacts::{BlobDeletion, DocumentBlobMaintenance};
use serde_json::json;

#[tokio::test]
async fn delete_surfaces_retry_after_on_429_without_inline_retry() {
    for list in [false, true] {
        for (header, expected) in [
            ("17", Some(Duration::from_secs(17))),
            ("invalid", None),
            ("-1", None),
        ] {
            let mut peer = Peer::start().await;
            let client = client(&peer, true);
            let task = operation(&client, list);
            authorize(&mut peer).await;
            peer.next()
                .await
                .respond_with_headers(429, &[("retry-after", header)]);
            let error = finish(task).await.unwrap_err();
            assert_eq!(error.error, DocumentError::Unavailable);
            assert_eq!(error.retry_after, expected);
            tokio::time::pause();
            tokio::time::advance(Duration::from_secs(120)).await;
            assert_eq!(peer.count(), 2);
            tokio::time::resume();
        }
    }
}

#[tokio::test]
async fn retry_after_is_capped_for_huge_seconds_and_far_future_dates() {
    let far_future = httpdate::fmt_http_date(
        std::time::SystemTime::now() + Duration::from_secs(365 * 24 * 60 * 60),
    );
    for list in [false, true] {
        let mut peer = Peer::start().await;
        let client = client(&peer, true);
        for (index, (header, seconds)) in [
            ("18446744073709551615", 3600),
            (far_future.as_str(), 3600),
            ("3601", 3600),
            ("3600", 3600),
            ("3599", 3599),
            ("0", 0),
            ("Thu, 01 Jan 1970 00:00:00 GMT", 0),
        ]
        .into_iter()
        .enumerate()
        {
            let task = operation(&client, list);
            if index == 0 {
                authorize(&mut peer).await;
            }
            peer.next()
                .await
                .respond_with_headers(429, &[("retry-after", header)]);
            let error = finish(task).await.unwrap_err();
            assert_eq!(error.error, DocumentError::Unavailable);
            assert_eq!(error.retry_after, Some(Duration::from_secs(seconds)));
            assert_eq!(peer.count(), index + 2);
        }
    }
}

#[tokio::test]
async fn maintenance_retry_after_date_is_request_local() {
    let mut peer = Peer::start().await;
    let client = client(&peer, true);
    let task = operation(&client, false);
    authorize(&mut peer).await;
    let held = peer.next().await;
    let second = operation(&client, true);
    peer.next()
        .await
        .respond_with_headers(429, &[("retry-after", "3")]);
    let now = std::time::SystemTime::now();
    let date = httpdate::fmt_http_date(now + Duration::from_secs(60));
    held.respond_with_headers(429, &[("retry-after", &date)]);
    let first = finish(task).await.unwrap_err();
    assert!(first.retry_after.unwrap() >= Duration::from_secs(58));
    assert!(first.retry_after.unwrap() <= Duration::from_secs(60));
    let second = finish(second).await.unwrap_err();
    assert_eq!(second.retry_after, Some(Duration::from_secs(3)));
    assert!(first.retry_after.unwrap() >= Duration::from_secs(58));
}

#[tokio::test]
async fn maintenance_redirects_never_contact_the_target() {
    for list in [false, true] {
        let mut peer = Peer::start().await;
        let foreign = Peer::start().await;
        let client = client(&peer, true);
        let task = operation(&client, list);
        authorize(&mut peer).await;
        peer.next()
            .await
            .respond_with_headers(307, &[("location", &foreign.origin)]);
        assert_eq!(
            finish(task).await.unwrap_err().error,
            DocumentError::Unavailable
        );
        assert_eq!(peer.count(), 2);
        assert_eq!(foreign.count(), 0);
    }
}

#[tokio::test]
async fn maintenance_listing_bounds_chunked_bodies_and_stalled_bodies() {
    for oversized in [false, true] {
        let mut peer = Peer::start().await;
        let client = client(&peer, true);
        let task = operation(&client, true);
        authorize(&mut peer).await;
        let request = peer.next().await;
        if oversized {
            let chunks = (0..5).map(|_| Ok::<_, std::io::Error>(vec![b'x'; 64 * 1024]));
            request.respond(
                200,
                axum::body::Body::from_stream(futures_util::stream::iter(chunks)),
            );
            assert_eq!(
                finish(task).await.unwrap_err().error,
                DocumentError::Corrupt
            );
        } else {
            request.stall_body();
            tokio::time::pause();
            tokio::time::advance(Duration::from_secs(12)).await;
            assert_eq!(
                finish(task).await.unwrap_err().error,
                DocumentError::Unavailable
            );
            tokio::time::resume();
        }
        assert_eq!(peer.count(), 2);
    }
}

#[tokio::test]
async fn maintenance_fixture_allows_rewriting_a_deleted_content_addressed_key() {
    let mut peer = Peer::start().await;
    let online = failure_tests::client(&peer);
    let maintenance = client(&peer, true);
    for pass in 0..2 {
        let task = failure_tests::operation(&online, true);
        if pass == 0 {
            failure_tests::authorize(&mut peer).await;
        }
        let request = peer.next().await;
        assert_eq!(request.method, Method::POST);
        request.json(json!({"data":{"key":failure_tests::key(),"contentLength":7,"generation":1,"digest":digest(b"fixture"),"fenceVersion":1}}));
        finish(task).await.unwrap();
        if pass == 0 {
            let copy = maintenance.clone();
            let task = tokio::spawn(async move { copy.delete(&failure_tests::key()).await });
            authorize(&mut peer).await;
            peer.next().await.respond(204, "");
            assert_eq!(finish(task).await.unwrap(), BlobDeletion::Deleted);
        }
    }
    assert_eq!(peer.count(), 5);
}
