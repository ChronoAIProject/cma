use super::*;
use cma_sdk::cmaeg::GatewayMediaEvent;

fn original() -> Value {
    json!({"event_id":"event-1","conversation_id":"conversation","text":"Original answer","message":null,"error":null,"response_id":"resp_parent","media_pending":true,"media_responses_url":"/v1/channels/channel/events/event-1/media-responses"})
}
fn media() -> Value {
    json!({"event_id":"event-1","conversation_id":"conversation","response_id":"resp_media","response_url":"/api/v2/responses/resp_media","parent_response_id":"resp_parent","delivery_id":"delivery","attachment_index":0,"kind":"original","status":"completed","text":"Attachment answer"})
}
fn terminal() -> Value {
    json!({"event_id":"event-1","conversation_id":"conversation","text":"Original answer","message":null,"error":null,"status":"completed","media_complete":true})
}
fn expect_media(server: &WireServer, body: String) {
    expect_stream_at(
        server,
        body,
        "/v1/channels/channel/events?mode=stream&media_responses=true",
    );
}

#[tokio::test]
async fn persisted_gateway_statuses_and_last_supported_attachment_slot_are_retained() {
    // media_response_store preserves these statuses; MediaArtifactClaim permits indices <32.
    for status in ["completed", "failed", "incomplete", "cancelled"] {
        let server = WireServer::new().await;
        let mut response = media();
        response["attachment_index"] = 31.into();
        response["status"] = status.into();
        if status != "completed" {
            response["text"] = "".into();
        }
        let mut final_outcome = terminal();
        if status != "completed" {
            final_outcome["status"] = "incomplete".into();
            final_outcome["media_complete"] = false.into();
        }
        expect_media(
            &server,
            frame("cmaeg.response.completed", original())
                + &frame("cmaeg.media.response", response)
                + &frame(
                    if status == "completed" {
                        "cmaeg.completed"
                    } else {
                        "cmaeg.failed"
                    },
                    final_outcome,
                ),
        );
        let mut stream = client(&server)
            .stream_event_with_media("channel", &event(), &EventOptions::default())
            .await
            .unwrap();
        assert!(matches!(
            stream.next().await,
            Some(Ok(GatewayMediaEvent::ResponseCompleted(_)))
        ));
        assert!(
            matches!(stream.next().await, Some(Ok(GatewayMediaEvent::MediaResponse(v))) if v.status == status && v.attachment_index == 31)
        );
        assert!(stream.next().await.unwrap().is_ok());
        assert!(stream.next().await.is_none());
        assert_eq!(server.request_count(), 1);
        server.assert_drained();
    }
}

#[tokio::test]
async fn explicit_media_stream_continues_after_original_completion_without_following_urls() {
    let server = WireServer::new().await;
    expect_media(
        &server,
        frame("cmaeg.response.completed", original())
            + &frame("cmaeg.media.response", media())
            + &frame("cmaeg.completed", terminal()),
    );
    let mut stream = client(&server)
        .stream_event_with_media("channel", &event(), &EventOptions::default())
        .await
        .unwrap();
    assert!(
        matches!(stream.next().await, Some(Ok(GatewayMediaEvent::ResponseCompleted(v))) if v.response_id == "resp_parent")
    );
    assert!(
        matches!(stream.next().await, Some(Ok(GatewayMediaEvent::MediaResponse(v))) if v.parent_response_id == "resp_parent" && v.text == "Attachment answer")
    );
    assert!(
        matches!(stream.next().await, Some(Ok(GatewayMediaEvent::Completed(v))) if v.media_complete && v.outcome.text.as_deref() == Some("Original answer"))
    );
    assert!(stream.next().await.is_none());
    assert_eq!(server.request_count(), 1);
    server.assert_drained();
}

#[tokio::test]
async fn explicit_media_stream_accepts_legacy_outcome_when_channel_has_no_media_work() {
    let server = WireServer::new().await;
    expect_media(&server, completed());
    let mut stream = client(&server)
        .stream_event_with_media("channel", &event(), &EventOptions::default())
        .await
        .unwrap();
    assert!(matches!(
        stream.next().await,
        Some(Ok(GatewayMediaEvent::Legacy(GatewayEvent::Completed(_))))
    ));
    assert!(stream.next().await.is_none());
    server.assert_drained();
}

#[tokio::test]
async fn pending_and_supplement_failure_close_without_replaying_or_overwriting_original_outcome() {
    for pending in [true, false] {
        let server = WireServer::new().await;
        let mut failure = terminal();
        failure["status"] = "incomplete".into();
        failure["media_complete"] = false.into();
        let last = if pending {
            frame(
                "cmaeg.media.pending",
                json!({"status":"pending","retrieval":"channel_activity"}),
            )
        } else {
            frame("cmaeg.failed", failure)
        };
        expect_media(
            &server,
            frame("cmaeg.response.completed", original()) + &last,
        );
        let mut stream = client(&server)
            .stream_event_with_media("channel", &event(), &EventOptions::default())
            .await
            .unwrap();
        assert!(matches!(
            stream.next().await,
            Some(Ok(GatewayMediaEvent::ResponseCompleted(_)))
        ));
        if pending {
            assert!(matches!(
                stream.next().await,
                Some(Ok(GatewayMediaEvent::MediaPending))
            ));
        } else {
            assert!(
                matches!(stream.next().await, Some(Ok(GatewayMediaEvent::Failed(v))) if !v.media_complete && v.outcome.error.is_none() && v.outcome.text.as_deref() == Some("Original answer"))
            );
        }
        assert!(stream.next().await.is_none());
        assert_eq!(server.request_count(), 1);
        server.assert_drained();
    }
}

#[tokio::test]
async fn malformed_media_frames_fail_closed_without_replay() {
    let mut cases = vec![];
    for (field, value) in [
        ("event_id", json!("wrong")),
        ("conversation_id", json!("wrong")),
        ("attachment_index", json!(32)),
        ("kind", json!("caption")),
        ("status", json!("queued")),
    ] {
        let mut value_body = media();
        value_body[field] = value;
        cases.push(frame("cmaeg.media.response", value_body));
    }
    cases.push(frame("cmaeg.response.failed", original()));
    cases.push(frame(
        "cmaeg.media.pending",
        json!({"status":"pending","retrieval":"untrusted_url"}),
    ));
    let mut wrong_terminal = terminal();
    wrong_terminal["media_complete"] = false.into();
    cases.push(frame("cmaeg.completed", wrong_terminal));
    for invalid in cases {
        let server = WireServer::new().await;
        expect_media(
            &server,
            frame("cmaeg.response.completed", original()) + &invalid,
        );
        let mut stream = client(&server)
            .stream_event_with_media("channel", &event(), &EventOptions::default())
            .await
            .unwrap();
        assert!(matches!(
            stream.next().await,
            Some(Ok(GatewayMediaEvent::ResponseCompleted(_)))
        ));
        assert!(matches!(
            stream.next().await,
            Some(Err(Error::Protocol { .. }))
        ));
        assert!(stream.next().await.is_none());
        assert_eq!(server.request_count(), 1);
        server.assert_drained();
    }
}

#[tokio::test]
async fn original_completion_followed_by_eof_is_not_complete() {
    let server = WireServer::new().await;
    expect_media(&server, frame("cmaeg.response.completed", original()));
    let mut stream = client(&server)
        .stream_event_with_media("channel", &event(), &EventOptions::default())
        .await
        .unwrap();
    assert!(matches!(
        stream.next().await,
        Some(Ok(GatewayMediaEvent::ResponseCompleted(_)))
    ));
    assert!(matches!(
        stream.next().await,
        Some(Err(Error::Transport { .. }))
    ));
    assert!(stream.next().await.is_none());
    assert_eq!(server.request_count(), 1);
    server.assert_drained();
}
