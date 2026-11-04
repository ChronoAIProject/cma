#[path = "support/wire.rs"]
mod wire;

use cma_sdk::{
    Error,
    cmaeg::{ChannelActivityEvent, ChannelActivityOptions, Cmaeg, StreamChannelActivityOptions},
};
use serde_json::{Value, json};
use wire::WireServer;

fn page() -> Value {
    json!({"cursor":"opaque/+?snapshot","retention_days":7,"limit":50,"has_more":false,"items":[{
        "event_id":"event-1","source":{"type":"custom"},"actor":{"id":"person","kind":"human"},
        "conversation":{"id":"conversation","kind":"private"},
        "occurred_at":"2026-10-27T01:00:00Z","received_at":"2026-10-27T01:00:01Z","consumers":[{
            "consumer_id":"consumer-1","state":"completed","display_name":"助手",

            "deliveries":[{"step":"reply","destination":"telegram","outcome":"delivered","observed_at":"2026-10-27T01:00:02Z",
                "confirmed_at":"2026-10-27T01:00:02Z","platform_message_id":"message-1","recipients":[{"kind":"chat","id":"chat-1"}]}]
        }]
    }]})
}
fn client(server: &WireServer) -> Cmaeg {
    let _ = (&server.client, &server.anonymous);
    Cmaeg::builder(&server.base_url)
        .unwrap()
        .bearer_token("sdk-fixture")
        .unwrap()
        .build()
        .unwrap()
}
fn frame(name: &str, page: &Value) -> String {
    format!(
        "event: {name}\r\nid: {}\r\ndata: {page}\r\n\r\n",
        page["cursor"].as_str().unwrap()
    )
}
fn stream(path: &str, body: &str) -> wire::Expected {
    wire::Expected {
        method: "GET".into(),
        path: path.into(),
        headers: wire::headers(true, "text/event-stream"),
        status: 200,
        response_headers: [("content-type".into(), "text/event-stream".into())].into(),
        chunks: Some(body.as_bytes().chunks(3).map(<[u8]>::to_vec).collect()),
        ..Default::default()
    }
}

#[test]
fn explicit_null_activity_observations_decode_without_losing_required_data() {
    use cma_sdk::cmaeg::types::{
        ActivityConsumer, ActivityDelivery, ActivityRecipient, ConsumerState,
    };
    let consumer: ActivityConsumer = serde_json::from_value(json!({
        "consumer_id": "default", "state": "received", "deliveries": [],
        "display_name": null, "agent_id": null, "provider": null, "conversation_id": null,
        "dispatched_at": null, "accepted_at": null, "produced_at": null,
        "completed_at": null, "response_id": null
    }))
    .unwrap();
    assert_eq!(consumer.consumer_id, "default");
    assert_eq!(consumer.state, ConsumerState::Received);
    assert!(consumer.deliveries.is_empty());
    assert!(
        [
            consumer.display_name,
            consumer.agent_id,
            consumer.provider,
            consumer.conversation_id,
            consumer.dispatched_at,
            consumer.accepted_at,
            consumer.produced_at,
            consumer.completed_at,
            consumer.response_id,
        ]
        .iter()
        .all(Option::is_none)
    );
    let recipient: ActivityRecipient = serde_json::from_value(json!({
        "kind": "custom", "id": "recipient", "thread_id": null
    }))
    .unwrap();
    assert_eq!(recipient.kind, "custom");
    assert_eq!(recipient.id, "recipient");
    assert!(recipient.thread_id.is_none());
    let delivery: ActivityDelivery = serde_json::from_value(json!({
        "step": "reply", "destination": "custom_stream", "outcome": "buffered",
        "observed_at": "2026-10-27T01:00:00Z", "confirmed_at": null,
        "platform_message_id": null, "recipients": [{"kind": "custom", "id": "recipient", "thread_id": null}]
    })).unwrap();
    assert_eq!(delivery.step, "reply");
    assert_eq!(delivery.destination, "custom_stream");
    assert_eq!(delivery.outcome, "buffered");
    assert_eq!(delivery.observed_at, "2026-10-27T01:00:00Z");
    assert!(delivery.confirmed_at.is_none());
    assert!(delivery.platform_message_id.is_none());
    assert!(delivery.recipients == vec![recipient]);
}

#[tokio::test]
async fn snapshots_keep_opaque_resume_owner_and_delivery_observations() {
    let server = WireServer::new().await;
    let client = client(&server);
    let page = page();
    let mut read = wire::json(
        "GET",
        "/v1/channels/opaque%2F+%3F/activity",
        None,
        200,
        page.clone(),
        true,
    );
    read.headers.insert("x-cmaeg-owner".into(), "org-id".into());
    server.expect(read);
    let received = client
        .get_channel_activity(
            "opaque/+?",
            &ChannelActivityOptions {
                owner: Some("org-id"),
            },
        )
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(received).unwrap(), page);
    let replacement =
        json!({"cursor":"replacement","items":[],"retention_days":7,"limit":50,"has_more":false});
    let mut expected = stream(
        "/v1/channels/opaque%2F+%3F/activity/stream?cursor=opaque%2F%2B%3Fsnapshot",
        &format!(
            ": keepalive\r\n\r\n{}{}",
            frame("activity", &page),
            frame("reset", &replacement)
        ),
    );
    expected
        .headers
        .insert("last-event-id".into(), "header-cursor".into());
    server.expect(expected);
    let mut events = client
        .stream_channel_activity(
            "opaque/+?",
            &StreamChannelActivityOptions {
                cursor: Some("opaque/+?snapshot"),
                last_event_id: Some("header-cursor"),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let ChannelActivityEvent::Activity(first) = events.next().await.unwrap().unwrap() else {
        panic!("expected Activity")
    };
    assert_eq!(serde_json::to_value(first).unwrap(), page);
    let ChannelActivityEvent::Reset(next) = events.next().await.unwrap().unwrap() else {
        panic!("expected reset")
    };
    assert_eq!(serde_json::to_value(next).unwrap(), replacement);
    assert!(events.next().await.is_none());
    assert_eq!(server.request_count(), 2);
    server.assert_drained();
}

#[tokio::test]
async fn malformed_snapshots_stop_without_acknowledging_or_reconnecting() {
    let server = WireServer::new().await;
    let client = client(&server);
    let page = page();
    let mut invalid = page.clone();
    invalid["limit"] = json!("50");
    let bodies = [
        frame("delta", &page),
        frame("activity", &page).replace("id: opaque/+?snapshot", "id: wrong"),
        format!("event: activity\ndata: {page}\n\n"),
        format!(
            "{}event: activity\ndata: {page}\n\n",
            frame("activity", &page)
        ),
        frame("activity", &page).trim_end().to_owned(),
        ": keepalive\n\n".into(),
        frame("activity", &invalid),
    ];
    for body in bodies {
        server.expect(stream("/v1/channels/channel/activity/stream", &body));
        let mut events = client
            .stream_channel_activity("channel", &Default::default())
            .await
            .unwrap();
        loop {
            match events.next().await {
                Some(Ok(_)) => continue,
                Some(Err(Error::Protocol { .. })) => break,
                _ => panic!("expected malformed Activity refusal"),
            }
        }
        assert!(events.next().await.is_none());
    }
    assert_eq!(server.request_count(), 7);
    server.assert_drained();
}

#[tokio::test]
async fn invalid_activity_authority_is_rejected_locally() {
    let server = WireServer::new().await;
    let client = client(&server);
    assert!(matches!(
        client
            .get_channel_activity(
                "channel",
                &ChannelActivityOptions {
                    owner: Some("bad owner")
                }
            )
            .await,
        Err(Error::Validation { .. })
    ));
    assert!(matches!(
        client
            .stream_channel_activity(
                "channel",
                &StreamChannelActivityOptions {
                    last_event_id: Some("bad\nheader"),
                    ..Default::default()
                }
            )
            .await,
        Err(Error::Validation { .. })
    ));
    assert_eq!(server.request_count(), 0);
}

#[tokio::test]
async fn retained_media_pages_and_activity_preserve_response_metadata_without_following_urls() {
    use cma_sdk::cmaeg::EventMediaResponseOptions;
    let server = WireServer::new().await;
    let mut expected = wire::json(
        "GET",
        "/v1/channels/channel-one/events/event-one/media-responses?cursor=0",
        None,
        200,
        json!({"items":[{"consumer_id":"consumer-1","attachment_index":0,"kind":"transcript",
            "response_id":"resp_supplement","response_url":"/api/v2/responses/resp_supplement",
            "status":"completed","delivery":"delivered","refusal":null}],"next_cursor":16}),
        true,
    );
    expected
        .headers
        .insert("x-cmaeg-owner".into(), "org-one".into());
    server.expect(expected);
    let result = client(&server)
        .get_event_media_responses(
            "channel-one",
            "event-one",
            &EventMediaResponseOptions {
                owner: Some("org-one"),
                cursor: Some(0),
            },
        )
        .await
        .unwrap();
    assert_eq!(result.next_cursor, Some(16));
    assert_eq!(
        result.items[0].response_id.as_deref(),
        Some("resp_supplement")
    );
    assert_eq!(server.request_count(), 1);
    let mut value = page();
    value["items"][0]["consumers"][0]["media_responses"] = json!([{"attachment_index":0,"kind":"transcript",
        "response_id":"resp_supplement","response_url":"/api/v2/responses/resp_supplement",
        "status":"completed","delivery":"delivered","observed_at":"2026-10-29T00:00:00Z"}]);
    server.expect(wire::json(
        "GET",
        "/v1/channels/channel-one/activity",
        None,
        200,
        value.clone(),
        true,
    ));
    let observed = client(&server)
        .get_channel_activity("channel-one", &ChannelActivityOptions::default())
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(observed).unwrap(), value);
    assert_eq!(server.request_count(), 2);
    server.assert_drained();
}
