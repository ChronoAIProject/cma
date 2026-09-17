use super::*;

fn original() -> DeliveryReceipt {
    DeliveryReceipt {
        nyxid_message_id: "retained-nyx-reply".into(),
        platform_message_id: Some("om_original_reply".into()),
    }
}
async fn edit(fixture: &Fixture, deadline: Duration) -> DeliveryOutcome {
    fixture
        .provider
        .post_edit(
            &original(),
            "Replacement text",
            &SecretValue::new("nyx_fixture".into()),
            tokio::time::Instant::now() + deadline,
        )
        .await
}
fn assert_one_edit(fixture: &Fixture) {
    assert_eq!(
        *fixture.state.paths.lock().unwrap(),
        ["/api/v1/channel-relay/reply/update"]
    );
    assert_eq!(
        *fixture.state.requests.lock().unwrap(),
        [json!({
            "message_id":"om_original_reply", "reply":{"text":"Replacement text"}
        })]
    );
}

#[tokio::test]
async fn edit_acknowledges_exact_original_platform_message_and_retains_receipt() {
    // NyxID UpdateReplyResponse uses Utc::now().to_rfc3339().
    for edited_at in [
        "2026-09-16T03:00:00Z",
        "2026-09-16T03:00:00.123456789+00:00",
        "2026-09-16T11:00:00.123+08:00",
    ] {
        let f = fixture(
            StatusCode::OK,
            json!({"upstream_message_id":"om_original_reply", "edited_at":edited_at}),
            Duration::ZERO,
        )
        .await;
        assert_eq!(
            edit(&f, Duration::from_secs(1)).await,
            DeliveryOutcome::Confirmed(original())
        );
        assert_one_edit(&f);
    }
}

#[tokio::test]
async fn malformed_or_mismatched_edit_acknowledgement_is_unknown_without_retry() {
    for body in [
        json!({"upstream_message_id":"other", "edited_at":"2026-09-16T03:00:00Z"}),
        json!({"upstream_message_id":"om_original_reply", "edited_at":"invalid"}),
        json!({"upstream_message_id":"om_original_reply", "edited_at":"1969-12-31T23:59:59.999+00:00"}),
        json!({"upstream_message_id":"om_original_reply", "edited_at":"2026-02-30T03:00:00+00:00"}),
        json!({"message_id":"new-outbound", "platform_message_id":"om_original_reply"}),
        json!({"upstream_message_id":"om_original_reply", "edited_at":"2026-09-16T03:00:00Z", "extra":true}),
    ] {
        let f = fixture(StatusCode::OK, body, Duration::ZERO).await;
        assert_eq!(
            edit(&f, Duration::from_secs(1)).await,
            DeliveryOutcome::Unknown(DeliveryFailure::ReceiptInvalid)
        );
        assert_one_edit(&f);
    }
}

#[tokio::test]
async fn failed_or_lost_edit_response_never_retries_the_provider_mutation() {
    for status in [
        StatusCode::BAD_REQUEST,
        StatusCode::UNAUTHORIZED,
        StatusCode::INTERNAL_SERVER_ERROR,
        StatusCode::TEMPORARY_REDIRECT,
    ] {
        let f = fixture(status, json!({"error":"private"}), Duration::ZERO).await;
        assert_eq!(
            edit(&f, Duration::from_secs(1)).await,
            DeliveryOutcome::Unknown(DeliveryFailure::ProviderFailure)
        );
        assert_one_edit(&f);
    }
    let f = fixture(StatusCode::OK, json!({}), Duration::from_secs(1)).await;
    assert_eq!(
        edit(&f, Duration::from_millis(100)).await,
        DeliveryOutcome::Unknown(DeliveryFailure::ResponseLost)
    );
    assert_one_edit(&f);
}

#[tokio::test]
async fn edit_without_confirmed_platform_identity_never_dispatches() {
    let f = fixture(StatusCode::OK, json!({}), Duration::ZERO).await;
    let mut receipt = original();
    receipt.platform_message_id = None;
    assert_eq!(
        f.provider
            .post_edit(
                &receipt,
                "Replacement",
                &SecretValue::new("nyx_fixture".into()),
                tokio::time::Instant::now() + Duration::from_secs(1)
            )
            .await,
        DeliveryOutcome::DefinitiveRejection(DeliveryFailure::ReceiptInvalid)
    );
    assert!(f.state.requests.lock().unwrap().is_empty());
}
