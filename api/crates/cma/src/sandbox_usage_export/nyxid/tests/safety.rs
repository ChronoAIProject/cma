use super::*;

#[tokio::test]
async fn paginated_holes_require_consistent_heads_and_progressing_pages() {
    let event = event();
    let mut first = cursor(&event);
    first["highest_received_sequence"] = "5".into();
    first["next_page"] = "opaque page/2".into();
    let mut second = first.clone();
    second["missing_ranges"] = json!([{"first":"3","last":"4"}]);
    second["next_page"] = serde_json::Value::Null;
    let server = Server::new(vec![
        issued("test", 3600),
        (200, first.to_string()),
        (200, second.to_string()),
    ])
    .await;
    let cursor = transport(&server)
        .cursor(&event.stream_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cursor.missing_ranges.len(), 2);
    assert!(server.calls.lock().unwrap()[2]
        .0
        .ends_with("?page=opaque+page%2F2"));
    second["highest_received_sequence"] = "6".into();
    let server = Server::new(vec![
        issued("test", 3600),
        (200, first.to_string()),
        (200, second.to_string()),
    ])
    .await;
    assert_eq!(
        transport(&server).cursor(&event.stream_id).await,
        Err(TransportError::CursorChanged)
    );
    let server = Server::new(vec![
        issued("test", 3600),
        (200, first.to_string()),
        (200, first.to_string()),
    ])
    .await;
    assert_eq!(
        transport(&server).cursor(&event.stream_id).await,
        Err(TransportError::Invalid)
    );
}

#[tokio::test]
async fn duplicate_fields_future_receipts_and_second_unauthorized_are_rejected() {
    let event = event();
    let duplicate = receipt(&event).replacen(
        r#""custody":"persisted""#,
        r#""custody":"persisted","custody":"persisted""#,
        1,
    );
    let mut future: serde_json::Value = serde_json::from_str(&receipt(&event)).unwrap();
    future["receipt"]["accepted_at_ms"] = i64::MAX.to_string().into();
    for body in [duplicate, future.to_string()] {
        let server = Server::new(vec![issued("test", 3600), (201, body)]).await;
        assert!(transport(&server).submit(&event).await.is_err());
    }
    let server = Server::new(vec![
        issued("first", 3600),
        (401, String::new()),
        issued("second", 3600),
        (401, String::new()),
    ])
    .await;
    assert_eq!(
        transport(&server).submit(&event).await,
        Err(TransportError::Unauthorized)
    );
    assert_eq!(server.calls.lock().unwrap().len(), 4);
}

#[tokio::test]
async fn retry_after_is_bounded_redirects_are_refused_and_response_bytes_are_limited() {
    use axum::{
        http::{HeaderMap, StatusCode},
        routing::any,
        Router,
    };
    for (status, retry, body, expected) in [
        (
            429,
            "999999999",
            String::new(),
            TransportError::RateLimited(3600),
        ),
        (429, "0", String::new(), TransportError::RateLimited(1)),
        (302, "", String::new(), TransportError::Invalid),
        (201, "", "x".repeat(512 * 1024 + 1), TransportError::Invalid),
    ] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap())
            .parse()
            .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", retry.parse().unwrap());
        headers.insert("location", "https://redirect.example/".parse().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().fallback(any(move || {
                    let headers = headers.clone();
                    let body = body.clone();
                    async move { (StatusCode::from_u16(status).unwrap(), headers, body) }
                })),
            )
            .await
            .unwrap();
        });
        struct Token;
        #[async_trait::async_trait]
        impl ProducerTokenProvider for Token {
            async fn token(
                &self,
            ) -> Result<crate::producer_token::ProducerToken, ProducerTokenError> {
                Ok(crate::producer_token::ProducerToken::new(
                    "fixture-bearer".into(),
                ))
            }
        }
        let client = NyxIdUsageTransport::with_tokens(base, Arc::new(Token)).unwrap();
        assert_eq!(client.submit(&event()).await, Err(expected));
        task.abort();
    }
}
