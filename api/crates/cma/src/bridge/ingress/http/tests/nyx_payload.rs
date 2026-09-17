use super::*;
use crate::bridge::ingress::capabilities::tests::fixtures::sign;
use layer2_core::bridge::partition::{NormalizedPartitionSource, PartitionAdmissionDecision};

// Mirrors NyxID 0ac096837c7436b05b71b374207a42320776715d:
// backend/src/services/channel_relay_service.rs CallbackPayload and
// build_callback_payload, with telegram.rs parse_webhook's unmodified Update.
// Empty attachments and absent reply/thread fields are omitted by the producer.
// Identifiers and message content are synthetic; the timestamp shape is chrono's
// DateTime<Utc>::to_rfc3339(), including fractional seconds and a numeric offset.
fn telegram_callback(timestamp: &str) -> HttpFixture {
    let mut fixture = HttpFixture::new(false);
    fixture.protocol.body = json!({
        "message_id": "nyx-message",
        "correlation_id": "signed-callback",
        "platform": "telegram",
        "agent": {"api_key_id": "selected-key", "name": "Fixture"},
        "conversation": {"id": "wildcard-route", "platform_id": "12345", "type": "private"},
        "sender": {"platform_id": "12345", "display_name": "Human"},
        "content": {"type": "text", "text": "Hey"},
        "timestamp": timestamp,
        "raw_platform_data": {
            "update_id": 99,
            "message": {
                "message_id": 4,
                "date": 1789620767,
                "from": {"id": 12345, "is_bot": false, "first_name": "Human", "language_code": "en"},
                "chat": {"id": 12345, "first_name": "Human", "type": "private"},
                "text": "Hey"
            }
        }
    });
    let now = layer2_core::seconds_from_rfc3339(&layer2_core::now_rfc3339()).unwrap();
    fixture.protocol.body["reply_token"] = json!(sign(&json!({
        "iss": "https://nyx.example", "aud": "channel-relay/reply",
        "iat": now, "exp": now + 1800, "jti": "fixture-reply",
        "token_type": "relay_reply", "api_key_id": "selected-key",
        "conversation_id": "wildcard-route", "inbound_message_id": "nyx-message",
        "platform": "telegram"
    })));
    fixture
}

#[tokio::test]
async fn nyx_telegram_payload_preserves_provider_timestamp_through_signed_http_admission() {
    for timestamp in [
        "2026-09-17T04:52:47.983+00:00",
        "2026-09-17T12:52:47.983+08:00",
        "2026-09-17T04:52:47.983Z",
        "2026-09-17T04:52:47+00:00",
        "2026-09-17T04:52:47Z",
    ] {
        let fixture = telegram_callback(timestamp);
        fixture.provider.commits.add_permits(1);
        let mut request = fixture.request();
        let header_timestamp = layer2_core::now_rfc3339().replace('Z', ".123+00:00");
        request
            .headers_mut()
            .insert("x-nyxid-timestamp", header_timestamp.parse().unwrap());
        let response = fixture.router().oneshot(request).await.unwrap();
        let status = response.status();
        let result = value(response).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{timestamp}: {result}");
        assert_eq!(result["receipt"]["admission"], "accepted");
        let rows = fixture.provider.rows.lock().await;
        assert_eq!(rows.len(), 1);
        let row = rows.values().next().unwrap();
        assert_eq!(row.envelope.timestamp, timestamp);
        assert_eq!(row.envelope.conversation_type, "private");
        assert_eq!(row.envelope.chat_id, "12345");
        assert_eq!(row.envelope.sender_id, "12345");
        assert_eq!(row.envelope.text.as_deref(), Some("Hey"));
        assert!(row.envelope.attachments.is_empty());
        assert!(row.reply_expires_at.is_some());

        // Durable recovery must validate the same provider contract as ingress.
        let recovered: StoredInbox =
            serde_json::from_value(serde_json::to_value(row).unwrap()).unwrap();
        let source =
            NormalizedPartitionSource::from_stored(&recovered, &fixture.protocol.bridge).unwrap();
        assert_eq!(source.provider_message_id(), Some("4"));
        assert!(matches!(
            source.admit(fixture.protocol.config()).unwrap(),
            PartitionAdmissionDecision::Admitted { .. }
        ));
    }
}

#[tokio::test]
async fn recovered_provider_timestamps_still_refuse_invalid_and_pre_epoch_values() {
    let fixture = telegram_callback("2026-09-17T04:52:47Z");
    let write = fixture.protocol.write().unwrap();
    for timestamp in [
        "2026-02-30T04:52:47.983+00:00",
        "1969-12-31T23:59:59.999+00:00",
        "2026-09-17T04:52:47",
    ] {
        let mut row = write.record().clone();
        row.envelope.timestamp = timestamp.into();
        assert!(NormalizedPartitionSource::from_stored(&row, &fixture.protocol.bridge).is_err());
    }
}
