use super::*;

fn frame_budget() -> Arc<ReadBudget> {
    Arc::new(ReadBudget {
        retain_frames: true,
        slots: Arc::new(Semaphore::new(1)),
        response_bytes: 8,
        timeout: Duration::from_secs(2),
    })
}

#[tokio::test]
async fn yielded_frames_and_slices_hold_admission_after_body_drop_or_eof() {
    for finish_body in [false, true] {
        let budget = frame_budget();
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let router = protected(
            Router::new()
                .route(
                    HISTORY,
                    get(move || {
                        let calls = observed.clone();
                        async move {
                            calls.fetch_add(1, Ordering::SeqCst);
                            "12345678"
                        }
                    }),
                )
                .route(
                    "/api/v1/agents/{id}/stop",
                    axum::routing::post(|| async { StatusCode::ACCEPTED }),
                )
                .route(
                    "/api/v1/documents/{id}/shares/{subject}",
                    axum::routing::delete(|| async { StatusCode::NO_CONTENT }),
                ),
            budget.clone(),
        );
        let response = request(&router, Method::GET, URL).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/plain; charset=utf-8"
        );
        let mut body = response.into_body().into_data_stream();
        let frame = body.next().await.unwrap().unwrap();
        let clone = frame.clone();
        let slice = clone.slice(2..4);
        if finish_body {
            assert!(body.next().await.is_none());
            assert_eq!(budget.slots.available_permits(), 0);
        }
        drop(body);
        drop(frame);
        drop(clone);
        assert_eq!(slice.as_ref(), b"34");
        assert_eq!(budget.slots.available_permits(), 0);
        let refused = request(&router, Method::GET, URL).await;
        assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(refused.headers()[header::RETRY_AFTER], "1");
        assert_eq!(refused.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            request(&router, Method::POST, "/api/v1/agents/a/stop")
                .await
                .status(),
            StatusCode::ACCEPTED
        );
        assert_eq!(
            request(&router, Method::DELETE, "/api/v1/documents/d/shares/s")
                .await
                .status(),
            StatusCode::NO_CONTENT
        );
        // Releasing the final slice from a different task returns the one slot.
        tokio::spawn(async move {
            drop(slice);
        })
        .await
        .unwrap();
        assert_eq!(budget.slots.available_permits(), 1);
        let recovered = request(&router, Method::GET, URL).await;
        assert_eq!(recovered.status(), StatusCode::OK);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        drop(recovered);
        assert_eq!(budget.slots.available_permits(), 1);
    }
}

#[tokio::test]
async fn collected_chunked_payload_keeps_its_permit_until_bytes_are_dropped() {
    let budget = frame_budget();
    let router = protected(
        Router::new().route(
            HISTORY,
            get(|| async {
                Body::from_stream(stream::iter([Ok::<_, Infallible>("1234"), Ok("5678")]))
            }),
        ),
        budget.clone(),
    );
    let response = request(&router, Method::GET, URL).await;
    let collected = to_bytes(response.into_body(), 8).await.unwrap();
    assert_eq!(collected, "12345678");
    assert_eq!(budget.slots.available_permits(), 0);
    assert_eq!(
        request(&router, Method::GET, URL).await.status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    drop(collected);
    assert_eq!(budget.slots.available_permits(), 1);
}

#[tokio::test]
async fn unread_body_drop_and_empty_denials_release_admission() {
    let budget = frame_budget();
    let nonempty = protected(
        Router::new().route(HISTORY, get(|| async { "payload" })),
        budget.clone(),
    );
    let response = request(&nonempty, Method::GET, URL).await;
    assert_eq!(budget.slots.available_permits(), 0);
    drop(response);
    assert_eq!(budget.slots.available_permits(), 1);
    for status in [
        StatusCode::OK,
        StatusCode::NO_CONTENT,
        StatusCode::NOT_FOUND,
    ] {
        let router = protected(
            Router::new().route(HISTORY, get(move || async move { status })),
            budget.clone(),
        );
        let response = request(&router, Method::GET, URL).await;
        assert_eq!(response.status(), status);
        assert_eq!(budget.slots.available_permits(), 1);
        assert!(to_bytes(response.into_body(), 8).await.unwrap().is_empty());
    }
}

#[tokio::test]
async fn enabled_collection_errors_and_oversize_release_before_any_payload() {
    let budget = frame_budget();
    for fail_stream in [false, true] {
        let router = protected(
            Router::new().route(
                HISTORY,
                get(move || async move {
                    Body::from_stream(stream::iter([
                        Ok("private"),
                        if fail_stream {
                            Err(std::io::Error::other("private source error"))
                        } else {
                            Ok("overflow")
                        },
                    ]))
                }),
            ),
            budget.clone(),
        );
        let response = request(&router, Method::GET, URL).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(budget.slots.available_permits(), 1);
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("private"));
    }
}

#[tokio::test(start_paused = true)]
async fn enabled_collection_timeout_and_cancel_return_capacity() {
    let budget = frame_budget();
    let entered = Arc::new(tokio::sync::Notify::new());
    let observed = entered.clone();
    let router = protected(
        Router::new().route(
            HISTORY,
            get(move || {
                let entered = observed.clone();
                async move {
                    Body::from_stream(stream::once(async move {
                        entered.notify_one();
                        std::future::pending::<Result<&'static str, Infallible>>().await
                    }))
                }
            }),
        ),
        budget.clone(),
    );
    let task_router = router.clone();
    let task = tokio::spawn(async move { request(&task_router, Method::GET, URL).await });
    entered.notified().await;
    assert_eq!(budget.slots.available_permits(), 0);
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(budget.slots.available_permits(), 1);
    assert_eq!(
        request(&router, Method::GET, URL).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(budget.slots.available_permits(), 1);
}

#[tokio::test]
async fn disabled_policy_preserves_body_owned_permit_compatibility() {
    let budget = budget(1, 8);
    let router = protected(
        Router::new().route(HISTORY, get(|| async { "payload" })),
        budget.clone(),
    );
    let mut body = request(&router, Method::GET, URL)
        .await
        .into_body()
        .into_data_stream();
    let frame = body.next().await.unwrap().unwrap();
    assert_eq!(budget.slots.available_permits(), 0);
    drop(body);
    assert_eq!(budget.slots.available_permits(), 1);
    assert_eq!(frame, "payload");
}
