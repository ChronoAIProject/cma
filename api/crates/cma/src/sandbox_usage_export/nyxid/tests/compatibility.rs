use super::*;

#[tokio::test]
async fn mutable_processing_observations_do_not_change_the_custody_receipt() {
    let event = event();
    let first: serde_json::Value = serde_json::from_str(&receipt(&event)).unwrap();
    let mut later = first.clone();
    later["processing"] = json!({"validation_status":"validation_conflict",
        "validation_reason":"hash_chain_conflict","owner_status":"owner_erased",
        "financial_status":"nonbillable_shadow"});
    let server = Server::new(vec![
        issued("test", 3600),
        (201, first.to_string()),
        (200, later.to_string()),
    ])
    .await;
    let client = transport(&server);
    let submitted = client.submit(&event).await.unwrap();
    let recovered = client.receipt(&event.event_id).await.unwrap().unwrap();
    recovered.validate(&event).unwrap();
    assert_eq!(submitted, recovered);
    for invalid in [
        first["receipt"].clone(),
        json!({"receipt":first["receipt"]}),
    ] {
        let server = Server::new(vec![issued("test", 3600), (200, invalid.to_string())]).await;
        assert_eq!(
            transport(&server).receipt(&event.event_id).await,
            Err(TransportError::Invalid)
        );
    }
}

#[tokio::test]
async fn changed_cursor_pages_restart_on_the_next_attempt_but_post_conflicts_hold() {
    let event = event();
    let mut first = cursor(&event);
    first["next_page"] = "opaque:1:64".into();
    let changed = r#"{"error":"conflict","error_code":1004,"message":"Usage cursor changed; restart pagination"}"#;
    let server = Server::new(vec![
        issued("test", 3600),
        (200, first.to_string()),
        (409, changed.into()),
        (200, cursor(&event).to_string()),
        (409, changed.into()),
    ])
    .await;
    let client = transport(&server);
    assert_eq!(
        client.cursor(&event.stream_id).await,
        Err(TransportError::CursorChanged)
    );
    assert!(client.cursor(&event.stream_id).await.unwrap().is_some());
    assert_eq!(client.submit(&event).await, Err(TransportError::Conflict));
    let calls = server.calls.lock().unwrap();
    assert!(calls[2].0.ends_with("?page=opaque%3A1%3A64"));
    assert!(!calls[3].0.contains('?'));
    assert_eq!(calls.len(), 5);
}

#[tokio::test]
async fn cursor_validation_keeps_custody_and_validation_heads_distinct() {
    let event = event();
    let mut valid = cursor(&event);
    valid["contiguous_custody_sequence"] = "2".into();
    valid["contiguous_validated_sequence"] = "1".into();
    valid["custody_chain_head"] = "a".repeat(64).into();
    valid["validated_chain_head"] = "b".repeat(64).into();
    valid["missing_ranges"] = json!([]);
    let server = Server::new(vec![issued("test", 3600), (200, valid.to_string())]).await;
    let observed = transport(&server)
        .cursor(&event.stream_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(observed.contiguous_sequence, "2");
    assert_eq!(observed.validated_sequence, "1");
    assert_eq!(observed.validated_head_digest, Some("b".repeat(64)));
    for (field, value) in [
        ("validated_chain_head", serde_json::Value::Null),
        ("validated_chain_head", "wrong".into()),
        ("contiguous_validated_sequence", "2".into()),
        ("contiguous_validated_sequence", "3".into()),
        ("contiguous_custody_sequence", "02".into()),
    ] {
        let mut invalid = valid.clone();
        invalid[field] = value;
        let server = Server::new(vec![issued("test", 3600), (200, invalid.to_string())]).await;
        assert_eq!(
            transport(&server).cursor(&event.stream_id).await,
            Err(TransportError::Invalid)
        );
    }
}

#[tokio::test]
async fn cursor_pages_bound_reads_and_refuse_overlapping_holes() {
    let event = event();
    let mut page = cursor(&event);
    page["next_page"] = "page-1".into();
    let mut overlap = page.clone();
    overlap["next_page"] = serde_json::Value::Null;
    let server = Server::new(vec![
        issued("test", 3600),
        (200, page.to_string()),
        (200, overlap.to_string()),
    ])
    .await;
    assert_eq!(
        transport(&server).cursor(&event.stream_id).await,
        Err(TransportError::Invalid)
    );
    let mut replies = vec![issued("test", 3600)];
    page["missing_ranges"] = json!([]);
    for index in 0..10 {
        page["next_page"] = format!("page-{index}").into();
        replies.push((200, page.to_string()));
    }
    let server = Server::new(replies).await;
    assert_eq!(
        transport(&server).cursor(&event.stream_id).await,
        Err(TransportError::Invalid)
    );
    assert_eq!(server.calls.lock().unwrap().len(), 10);
}
