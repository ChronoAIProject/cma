//! Inline `data:` attachments read from the verified custom event.
use super::support::{activity, event, far, request, resolved};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg::media::{
    source::{inline::InlineSource, MediaSource},
    FetchError,
};
use cmaeg_core::{
    activity::{Activity, ActivityKind, AttachmentSource, SourceKind},
    seal::ReplyAddress,
};
use futures_util::StreamExt;
use serde_json::{json, Value};

fn event_activity(data: Value) -> Activity {
    activity(
        SourceKind::Custom,
        Some(ActivityKind::Event {
            ce: serde_json::from_value(json!({
                "specversion": "1.0",
                "id": "event-1",
                "source": "/fixture",
                "type": "fixture.created",
                "data": data,
            }))
            .unwrap(),
            attachments: Vec::new(),
        }),
    )
}

fn data_uri(bytes: &[u8]) -> String {
    format!("data:image/png;base64,{}", STANDARD.encode(bytes))
}

async fn read(activity: &Activity, pointer: &str, cap: u64) -> Result<Vec<u8>, FetchError> {
    let channel = resolved(|_| {});
    let event = event(ReplyAddress::None);
    let locator = AttachmentSource::Inline {
        pointer: pointer.into(),
    };
    InlineSource
        .fetch(request(&channel, &event, activity, &locator, cap, far()))
        .await
        .map(|body| {
            assert!(!body.truncated);
            body.bytes.to_vec()
        })
}

#[tokio::test]
async fn an_inline_payload_is_decoded_from_the_event() {
    let activity = event_activity(json!({"attachments": [{"url": data_uri(b"png-bytes")}]}));
    assert_eq!(
        read(&activity, "/attachments/0/url", 1024).await.unwrap(),
        b"png-bytes"
    );
    let with_parameters = event_activity(json!({"a": "data:text/plain;charset=utf-8;base64,aGk="}));
    assert_eq!(read(&with_parameters, "/a", 1024).await.unwrap(), b"hi");
}

#[tokio::test]
async fn an_oversized_payload_is_too_large() {
    let at_limit = event_activity(json!({"a": data_uri(&vec![1; 196_608])}));
    assert_eq!(read(&at_limit, "/a", 1 << 20).await.unwrap().len(), 196_608);
    let over_limit = event_activity(json!({"a": data_uri(&vec![1; 196_609])}));
    assert_eq!(
        read(&over_limit, "/a", 1 << 20).await,
        Err(FetchError::TooLarge)
    );
    let over_cap = event_activity(json!({"a": data_uri(&[1; 100])}));
    assert_eq!(read(&over_cap, "/a", 99).await, Err(FetchError::TooLarge));
    assert_eq!(read(&over_cap, "/a", 100).await.unwrap().len(), 100);
}

#[tokio::test]
async fn a_missing_or_malformed_value_is_not_found() {
    let activity = event_activity(json!({
        "number": 7,
        "link": "https://media.example/a.png",
        "plain": "data:image/png,AAAA",
        "bad": "data:image/png;base64,***=",
        "unpadded": "data:image/png;base64,aGk",
    }));
    for pointer in [
        "/missing",
        "/number",
        "/link",
        "/plain",
        "/bad",
        "/unpadded",
    ] {
        assert_eq!(
            read(&activity, pointer, 1024).await,
            Err(FetchError::NotFound),
            "{pointer}"
        );
    }
    let without_data = activity_without_data();
    assert_eq!(
        read(&without_data, "/a", 1024).await,
        Err(FetchError::NotFound)
    );
}

fn activity_without_data() -> Activity {
    activity(
        SourceKind::Custom,
        Some(ActivityKind::Event {
            ce: serde_json::from_value(json!({
                "specversion": "1.0",
                "id": "event-1",
                "source": "/fixture",
                "type": "fixture.created",
            }))
            .unwrap(),
            attachments: Vec::new(),
        }),
    )
}

#[tokio::test]
async fn a_non_event_activity_is_not_retrievable() {
    let message = activity(SourceKind::Custom, None);
    assert_eq!(
        read(&message, "/attachments/0/url", 1024).await,
        Err(FetchError::NotRetrievable)
    );
}

#[tokio::test]
async fn open_yields_the_payload_as_one_chunk() {
    let activity = event_activity(json!({"a": data_uri(b"png-bytes")}));
    let channel = resolved(|_| {});
    let event = event(ReplyAddress::None);
    let locator = AttachmentSource::Inline {
        pointer: "/a".into(),
    };
    let opened = InlineSource
        .open(request(&channel, &event, &activity, &locator, 1024, far()))
        .await
        .unwrap();
    assert_eq!(opened.content_length, Some(9));
    let chunks: Vec<_> = opened.stream.collect().await;
    assert_eq!(chunks.len(), 1);
    assert_eq!(&chunks[0].as_ref().unwrap()[..], b"png-bytes");
}
