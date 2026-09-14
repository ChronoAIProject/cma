use super::*;

#[test]
fn only_explicit_transient_statuses_are_retryable() {
    for status in 100..=599 {
        assert_eq!(
            transient(StatusCode::from_u16(status).unwrap()),
            matches!(status, 429 | 500 | 502 | 503 | 504)
        );
    }
}

#[test]
fn retry_after_supports_seconds_dates_and_unbounded_delays_without_early_retry() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let future = httpdate::fmt_http_date(now + Duration::from_secs(75));
    let past = httpdate::fmt_http_date(now - Duration::from_secs(75));
    for (value, expected) in [
        ("0", Duration::ZERO),
        ("37", Duration::from_secs(37)),
        (" 37 ", Duration::from_secs(37)),
        (future.as_str(), Duration::from_secs(75)),
        (past.as_str(), Duration::ZERO),
        ("invalid", Duration::ZERO),
        ("-2", Duration::ZERO),
        ("1.5", Duration::ZERO),
        ("184467440737095516150000", Duration::from_secs(u64::MAX)),
    ] {
        let mut headers = header::HeaderMap::new();
        headers.insert(header::RETRY_AFTER, value.parse().unwrap());
        assert_eq!(retry_after(&headers, now), expected);
    }
    assert_eq!(retry_after(&header::HeaderMap::new(), now), Duration::ZERO);
}

#[test]
fn cloned_request_bodies_share_the_owned_part_allocation() {
    let bytes = vec![0x35; PART_BYTES];
    let original = bytes.as_ptr();
    let request = reqwest::Client::new()
        .post("http://127.0.0.1/part")
        .body(bytes)
        .build()
        .unwrap();
    assert_eq!(
        request.body().unwrap().as_bytes().unwrap().as_ptr(),
        original
    );
    for _ in 0..ATTEMPTS {
        let cloned = request.try_clone().unwrap();
        assert_eq!(
            cloned.body().unwrap().as_bytes().unwrap().as_ptr(),
            original
        );
        assert_eq!(cloned.body().unwrap().as_bytes().unwrap().len(), PART_BYTES);
    }
}

#[tokio::test(start_paused = true)]
async fn retry_wait_respects_server_delay_and_remains_inside_total_upload_budget() {
    let start = tokio::time::Instant::now();
    wait_for_retry(1, Duration::from_secs(75)).await;
    assert_eq!(start.elapsed(), Duration::from_secs(75));
    let start = tokio::time::Instant::now();
    wait_for_retry(2, Duration::ZERO).await;
    assert_eq!(start.elapsed(), Duration::from_secs(2));
    for delay in [UPLOAD_LIFETIME, Duration::from_secs(u64::MAX)] {
        let start = tokio::time::Instant::now();
        assert!(tokio::time::timeout(UPLOAD_LIFETIME, async {
            tokio::time::sleep(Duration::from_secs(500)).await;
            wait_for_retry(1, delay).await;
        })
        .await
        .is_err());
        assert_eq!(start.elapsed(), UPLOAD_LIFETIME);
    }
}
