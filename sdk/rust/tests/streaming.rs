#[path = "support/wire.rs"]
mod wire;

use cma_sdk::{
    Error, agui::ReplayOptions, launches::LaunchReplayOptions, responses::CreateResponseRequest,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use wire::{Expected, WireServer};

fn sse(path: &str, chunks: Vec<Vec<u8>>) -> Expected {
    Expected {
        method: "GET".into(),
        path: path.into(),
        headers: wire::headers(true, "text/event-stream"),
        status: 200,
        response_headers: BTreeMap::from([(
            "content-type".into(),
            "text/event-stream; charset=utf-8".into(),
        )]),
        chunks: Some(chunks),
        ..Default::default()
    }
}

fn frame(id: Option<&str>, event: Option<&str>, data: Value) -> Vec<u8> {
    format!(
        "{}{}data: {data}\r\n\r\n",
        id.map(|value| format!("id: {value}\r\n"))
            .unwrap_or_default(),
        event
            .map(|value| format!("event: {value}\r\n"))
            .unwrap_or_default()
    )
    .into_bytes()
}

fn agent(kind: &str, live: bool) -> Value {
    let mut value = json!({"type":kind,"threadId":"agt_one","runId":"1"});
    if live {
        value["cma:live"] = json!(true);
    }
    value
}

#[tokio::test]
async fn agui_acknowledges_only_new_complete_durable_records_across_split_crlf_and_utf8() {
    let server = WireServer::new().await;
    let mut bytes = b": heartbeat\r\n\r\n".to_vec();
    bytes.extend(frame(Some("1"), None, agent("RUN_STARTED", false)));
    let mut live = agent("TEXT_MESSAGE_CONTENT", true);
    live["delta"] = json!("caf\u{00e9}");
    bytes.extend(frame(None, None, live));
    bytes.extend(frame(None, None, agent("TEXT_MESSAGE_START", false)));
    bytes.extend(frame(Some("2"), None, agent("TEXT_MESSAGE_END", false)));
    let chunks = bytes.chunks(1).map(<[u8]>::to_vec).collect();
    server.expect(sse(
        "/api/v1/agui/events?thread_id=agt_one&follow=false",
        chunks,
    ));
    let mut stream = server
        .client
        .agui()
        .events("agt_one", ReplayOptions::default())
        .await
        .unwrap();
    assert_eq!(stream.next().await.unwrap().unwrap().cursor, Some(1));
    assert_eq!(stream.next().await.unwrap().unwrap().cursor, None);
    assert_eq!(stream.next().await.unwrap().unwrap().cursor, None);
    assert_eq!(stream.next().await.unwrap().unwrap().cursor, Some(2));
    assert_eq!(stream.cursor(), 2);
    assert!(stream.next().await.is_none());
    assert_eq!(server.request_count(), 1);
    server.assert_drained();
}

#[tokio::test]
async fn agui_rejects_wrong_identity_unknown_tags_names_and_regressing_cursors() {
    for (id, event_name, data) in [
        (
            "2",
            None,
            json!({"type":"RUN_STARTED","threadId":"foreign","runId":"1"}),
        ),
        ("2", None, agent("UNRECOGNIZED", false)),
        ("2", Some("unexpected"), agent("RUN_STARTED", false)),
        ("1", None, agent("RUN_STARTED", false)),
        ("2", None, agent("TEXT_MESSAGE_CONTENT", true)),
    ] {
        let server = WireServer::new().await;
        server.expect(sse(
            "/api/v1/agui/events?thread_id=agt_one&follow=false&starting_after=1",
            vec![frame(Some(id), event_name, data)],
        ));
        let mut stream = server
            .client
            .agui()
            .events(
                "agt_one",
                ReplayOptions {
                    after: Some(1),
                    follow: false,
                },
            )
            .await
            .unwrap();
        assert!(matches!(
            stream.next().await.unwrap(),
            Err(Error::Protocol { .. })
        ));
        assert_eq!(stream.cursor(), 1);
        assert!(stream.next().await.is_none());
        server.assert_drained();
    }
}

#[tokio::test]
async fn launch_stream_preserves_exact_identity_cursor_and_finite_eof() {
    let server = WireServer::new().await;
    let event = json!({"launch_id":"lch_one","seq":4,"appended_at":"2026-09-07T00:00:00Z","kind":{"type":"created"}});
    server.expect(sse(
        "/api/v1/agui/launches/lch_one/events?starting_after=3&limit=20&follow=false",
        vec![frame(Some("4"), None, event)],
    ));
    let mut stream = server
        .client
        .launches()
        .stream(
            "lch_one",
            LaunchReplayOptions {
                after: Some(3),
                limit: Some(20),
                follow: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(stream.next().await.unwrap().unwrap().seq, 4);
    assert_eq!(stream.cursor(), 4);
    assert!(stream.next().await.is_none());
    server.assert_drained();
}

#[tokio::test]
async fn launch_stream_rejects_an_inherited_or_mismatched_cursor_without_advancing() {
    for id in [None, Some("2"), Some("01")] {
        let server = WireServer::new().await;
        let event = json!({"launch_id":"lch_one","seq":1,"appended_at":"2026-09-07T00:00:00Z","kind":{"type":"created"}});
        server.expect(sse(
            "/api/v1/agui/launches/lch_one/events?follow=false",
            vec![frame(id, None, event)],
        ));
        let mut stream = server
            .client
            .launches()
            .stream("lch_one", LaunchReplayOptions::default())
            .await
            .unwrap();
        assert!(matches!(
            stream.next().await.unwrap(),
            Err(Error::Protocol { .. })
        ));
        assert_eq!(stream.cursor(), 0);
        assert!(stream.next().await.is_none());
        server.assert_drained();
    }
}

#[tokio::test]
async fn malformed_utf8_json_and_oversized_frames_stop_the_reader() {
    for bytes in [
        b"data: \xff\n\n".to_vec(),
        b"data: invalid\n\n".to_vec(),
        vec![b'x'; 1024 * 1024 + 1],
    ] {
        let server = WireServer::new().await;
        server.expect(sse(
            "/api/v1/agui/events?thread_id=agt_one&follow=false",
            vec![bytes],
        ));
        let mut stream = server
            .client
            .agui()
            .events("agt_one", ReplayOptions::default())
            .await
            .unwrap();
        assert!(matches!(
            stream.next().await.unwrap(),
            Err(Error::Protocol { .. })
        ));
        assert!(stream.next().await.is_none());
        server.assert_drained();
    }
}

fn response_expectation(chunks: Vec<Vec<u8>>) -> Expected {
    let mut expected = sse("/api/v1/responses", chunks);
    expected.method = "POST".into();
    expected.body = Some(json!({"conversation":"agt_one","input":"hello","stream":true}));
    expected
        .headers
        .insert("content-type".into(), "application/json".into());
    expected
}

#[tokio::test]
async fn responses_requires_exact_sequence_and_terminal_event_without_replay_ids() {
    let server = WireServer::new().await;
    server.expect(response_expectation(vec![
        frame(
            None,
            Some("response.created"),
            json!({"type":"response.created","sequence_number":0}),
        ),
        frame(
            None,
            Some("response.completed"),
            json!({"type":"response.completed","sequence_number":1}),
        ),
    ]));
    let mut stream = server
        .client
        .responses()
        .create(&CreateResponseRequest::new("agt_one", "hello"))
        .await
        .unwrap();
    assert_eq!(stream.next().await.unwrap().unwrap().sequence_number, 0);
    assert_eq!(stream.next().await.unwrap().unwrap().sequence_number, 1);
    assert!(stream.next().await.is_none());
    server.assert_drained();
    for (id, name, data) in [
        (
            Some("1"),
            "response.created",
            json!({"type":"response.created","sequence_number":0}),
        ),
        (
            None,
            "response.created",
            json!({"type":"response.created","sequence_number":1}),
        ),
        (
            None,
            "response.completed",
            json!({"type":"response.created","sequence_number":0}),
        ),
    ] {
        let server = WireServer::new().await;
        server.expect(response_expectation(vec![frame(id, Some(name), data)]));
        let mut stream = server
            .client
            .responses()
            .create(&CreateResponseRequest::new("agt_one", "hello"))
            .await
            .unwrap();
        assert!(matches!(
            stream.next().await.unwrap(),
            Err(Error::Protocol { .. })
        ));
        assert!(stream.next().await.is_none());
        server.assert_drained();
    }
}

#[tokio::test]
async fn responses_early_eof_is_ambiguous_transport_failure_and_never_retries() {
    let server = WireServer::new().await;
    server.expect(response_expectation(vec![frame(
        None,
        Some("response.created"),
        json!({"type":"response.created","sequence_number":0}),
    )]));
    let mut stream = server
        .client
        .responses()
        .create(&CreateResponseRequest::new("agt_one", "hello"))
        .await
        .unwrap();
    stream.next().await.unwrap().unwrap();
    assert!(matches!(
        stream.next().await.unwrap(),
        Err(Error::Transport { .. })
    ));
    assert!(stream.next().await.is_none());
    server.assert_drained();
}

#[tokio::test]
async fn stream_problem_status_and_correlation_are_preserved_before_observation() {
    let server = WireServer::new().await;
    let mut expected = sse("/api/v1/agui/events?thread_id=agt_one&follow=false", vec![]);
    expected.status = 503;
    expected.chunks = None;
    expected.response = serde_json::to_vec(
        &json!({"status":503,"code":"store_unavailable","detail":"Unavailable","limit":12}),
    )
    .unwrap();
    expected
        .response_headers
        .insert("x-request-id".into(), "request-stream".into());
    expected
        .response_headers
        .insert("retry-after".into(), "3".into());
    server.expect(expected);
    let Err(error) = server
        .client
        .agui()
        .events("agt_one", ReplayOptions::default())
        .await
    else {
        panic!("expected refusal")
    };
    assert!(
        matches!(error, Error::Api { status:503, request_id:Some(ref request), retry_after:Some(delay), .. }
        if request == "request-stream" && delay.as_secs() == 3)
    );
    assert_eq!(error.extension("code"), None);
    server.assert_drained();
}

#[tokio::test]
async fn auth_dependency_failure_retains_correlation_and_public_signup_omits_credentials() {
    let server = WireServer::new().await;
    let mut expected = wire::json(
        "GET",
        "/auth/session",
        None,
        503,
        json!({"authenticated":false,"code":"store_failed"}),
        true,
    );
    expected
        .response_headers
        .insert("x-request-id".into(), "request-auth".into());
    expected
        .response_headers
        .insert("retry-after".into(), "2".into());
    server.expect(expected);
    let error = server.client.auth().read_session().await.unwrap_err();
    assert!(
        matches!(error, Error::Api { status:503, request_id:Some(ref id), retry_after:Some(delay), .. } if id == "request-auth" && delay.as_secs() == 2)
    );
    server.expect(wire::json("GET", "/auth/signup-options", None, 200, json!({"social_providers":[],"email_signup":false,"invite_code_required":true,"register_available":false}), false));
    server.client.auth().read_signup_options().await.unwrap();
    let _ = (&server.anonymous, &server.base_url);
    server.assert_drained();
}

#[tokio::test]
async fn authenticated_bot_owner_capability_is_preserved() {
    use cma_sdk::auth::{AuthSession, Capability};
    let server = WireServer::new().await;
    server.expect(wire::json(
        "GET",
        "/auth/session",
        None,
        200,
        json!({"authenticated":true,"subject":"bot-owner","visitor":false,
            "capabilities":["manage_own_bots","manage_own_profiles"]}),
        true,
    ));
    let AuthSession::SignedIn(identity) = server.client.auth().read_session().await.unwrap() else {
        panic!("expected authenticated owner")
    };
    assert_eq!(identity.subject, "bot-owner");
    assert_eq!(
        identity.capabilities,
        vec![Capability::ManageOwnBots, Capability::ManageOwnProfiles]
    );
    server.assert_drained();
}

#[test]
fn response_input_models_cover_all_supported_single_message_forms_and_reject_other_roles() {
    for input in [
        json!("hello"),
        json!([{"role":"user","content":"hello"}]),
        json!([{"type":"message","role":"user","content":[{"type":"input_text","text":"hello"}]}]),
    ] {
        let value = json!({"conversation":"agt_one","input":input,"stream":true});
        let request: CreateResponseRequest = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(request).unwrap(), value);
    }
    for input in [
        json!([]),
        json!([{"role":"assistant","content":"hello"}]),
        json!([{"role":"user","content":"one"},{"role":"user","content":"two"}]),
    ] {
        assert!(
            serde_json::from_value::<CreateResponseRequest>(
                json!({"conversation":"agt_one","input":input,"stream":true})
            )
            .is_err()
        );
    }
}
