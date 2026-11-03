#[path = "responses_v2/activity.rs"]
mod activity;
#[path = "responses_v2/binding.rs"]
mod binding;
#[path = "responses_v2/fixture.rs"]
mod fixture;
#[path = "support/wire.rs"]
mod wire;
use cma_sdk::{
    Error,
    responses_v2::{CreateResponseRequest, ResponseEvent, ResponseEventPayload},
};
use fixture::{events, expected};
use serde_json::{Value, json};
use wire::WireServer;

#[tokio::test]
async fn suggestion_sends_exact_text_and_answer_scope_with_the_retained_key() {
    let server = WireServer::new().await;
    let body = json!({"conversation":"agt_fixture", "input":"Add criteria.\nKeep limits.", "stream":true,
        "answer_context":{"turn_sequence":2,"answer_sequence":4}});
    let request: CreateResponseRequest = serde_json::from_value(body.clone()).unwrap();
    let mut reply = expected("/api/v2/responses", Some(body), fixture::wire(&events()));
    reply
        .headers
        .insert("idempotency-key".into(), "suggestion-original".into());
    server.expect(reply);
    let stream = server
        .client
        .responses_v2()
        .create(&request, Some("suggestion-original"))
        .await
        .unwrap();
    assert_eq!(stream.receipt().command_id, "cmd_contract");
    server.assert_drained();
}

#[tokio::test]
async fn both_aliases_retain_the_receipt_and_key_before_split_utf8_events() {
    let server = WireServer::new().await;
    let request = CreateResponseRequest::new("agt_fixture", "Hello");
    for path in ["/api/v2/responses", "/api/v2/agents/agt_fixture/responses"] {
        let bytes = String::from_utf8(fixture::wire(&events()))
            .unwrap()
            .replace('\n', "\r\n")
            .into_bytes();
        let mut reply = expected(path, Some(serde_json::to_value(&request).unwrap()), vec![]);
        reply
            .headers
            .insert("idempotency-key".into(), "retained-key".into());
        reply.chunks = Some(bytes.chunks(1).map(|chunk| chunk.to_vec()).collect());
        server.expect(reply);
        let mut stream = if path == "/api/v2/responses" {
            server
                .client
                .responses_v2()
                .create(&request, Some("retained-key"))
                .await
                .unwrap()
        } else {
            server
                .client
                .responses_v2()
                .create_for_agent("agt_fixture", &request, Some("retained-key"))
                .await
                .unwrap()
        };
        assert_eq!(stream.receipt().response_id, "resp_contract");
        assert_eq!(stream.receipt().command_id, "cmd_contract");
        for event in events() {
            assert_eq!(
                stream.next().await.unwrap().unwrap(),
                serde_json::from_value::<ResponseEvent>(event).unwrap()
            );
        }
        assert!(stream.next().await.is_none());
    }
    assert_eq!(server.request_count(), 2);
    server.assert_drained();
}
#[tokio::test]
async fn explicit_replay_accepts_every_cursor_including_mid_content_and_completed_eof() {
    let server = WireServer::new().await;
    let events = events();
    for cursor in 0..events.len() {
        let tail = &events[cursor + 1..];
        server.expect(expected(
            &format!("/api/v2/responses/resp_contract/events?starting_after={cursor}"),
            None,
            fixture::wire(tail),
        ));
        let mut stream = server
            .client
            .responses_v2()
            .replay("resp_contract", Some(cursor as u64))
            .await
            .unwrap();
        for event in tail {
            assert_eq!(
                stream.next().await.unwrap().unwrap(),
                serde_json::from_value::<ResponseEvent>(event.clone()).unwrap()
            );
        }
        assert!(stream.next().await.is_none());
    }
    server.assert_drained();
}
#[tokio::test]
async fn premature_eof_keeps_receipt_and_recovery_get_never_resubmits_input() {
    let server = WireServer::new().await;
    let request = CreateResponseRequest::new("agt_fixture", "Hello");
    server.expect(expected(
        "/api/v2/responses",
        Some(serde_json::to_value(&request).unwrap()),
        fixture::wire(&events()[..5]),
    ));
    let mut stream = server
        .client
        .responses_v2()
        .create(&request, None)
        .await
        .unwrap();
    for _ in 0..5 {
        stream.next().await.unwrap().unwrap();
    }
    assert!(matches!(
        stream.next().await,
        Some(Err(Error::Transport { .. }))
    ));
    assert_eq!(stream.receipt().response_id, "resp_contract");
    server.expect(expected(
        "/api/v2/responses/resp_contract/events?starting_after=4",
        None,
        fixture::wire(&events()[5..]),
    ));
    let mut resumed = server
        .client
        .responses_v2()
        .replay("resp_contract", Some(4))
        .await
        .unwrap();
    for event in &events()[5..] {
        assert_eq!(
            resumed.next().await.unwrap().unwrap(),
            serde_json::from_value::<ResponseEvent>(event.clone()).unwrap()
        );
    }
    assert!(resumed.next().await.is_none());
    assert_eq!(server.request_count(), 2);
}
#[tokio::test]
async fn connection_error_has_no_explicit_replay_id_and_cannot_fabricate_failure() {
    let server = WireServer::new().await;
    let error = json!({"type":"error","sequence_number":1,"code":"response_observation_unavailable","message":"Reconnect","param":null});
    for explicit in [false, true] {
        let mut wire = fixture::wire(&events()[..1]);
        if explicit {
            wire.extend_from_slice(b"id: 0\n");
        }
        wire.extend(fixture::wire(std::slice::from_ref(&error)));
        server.expect(expected(
            "/api/v2/responses/resp_contract/events",
            None,
            wire,
        ));
        let mut stream = server
            .client
            .responses_v2()
            .replay("resp_contract", None)
            .await
            .unwrap();
        stream.next().await.unwrap().unwrap();
        if explicit {
            assert!(matches!(
                stream.next().await,
                Some(Err(Error::Protocol { .. }))
            ));
        } else {
            assert!(matches!(
                stream.next().await.unwrap().unwrap().payload,
                ResponseEventPayload::Error { .. }
            ));
            assert!(matches!(
                stream.next().await,
                Some(Err(Error::Transport { .. }))
            ));
        }
        assert!(stream.next().await.is_none());
    }
}
#[tokio::test]
async fn output_identity_index_text_status_and_terminal_mutations_fail_closed() {
    let server = WireServer::new().await;
    let mutations = [
        (0, "/response/id", json!("resp_foreign")),
        (0, "/response/conversation/id", json!("agt_foreign")),
        (0, "/response/status", json!("in_progress")),
        (2, "/output_index", json!(2)),
        (3, "/item_id", json!("other")),
        (4, "/content_index", json!(1)),
        (5, "/text", json!("changed")),
        (6, "/part/text", json!("changed")),
        (7, "/item/status", json!("in_progress")),
        (8, "/response/output", json!([])),
    ];
    let request = CreateResponseRequest::new("agt_fixture", "Hello");
    for (index, pointer, value) in mutations {
        let mut events = events();
        *events[index].pointer_mut(pointer).unwrap() = value;
        server.expect(expected(
            "/api/v2/responses",
            Some(serde_json::to_value(&request).unwrap()),
            fixture::wire(&events),
        ));
        let mut stream = server
            .client
            .responses_v2()
            .create(&request, None)
            .await
            .unwrap();
        loop {
            match stream.next().await.expect("mutation must fail") {
                Ok(_) => {}
                Err(Error::Protocol { .. }) => break,
                other => panic!("unexpected {other:?}"),
            }
        }
        assert!(stream.next().await.is_none());
    }
}
#[tokio::test]
async fn explicit_event_id_cannot_be_missing_wrong_or_inherited() {
    let server = WireServer::new().await;
    let base = String::from_utf8(fixture::wire(&events())).unwrap();
    for wire in [
        base.replacen("id: 0\n", "", 1),
        base.replacen("id: 0", "id: 9", 1),
        base.replacen("id: 1\n", "", 1),
        base.replacen("event: response.created", "event: response.failed", 1),
        format!(
            "{base}{}",
            String::from_utf8(fixture::wire(&events()[8..])).unwrap()
        ),
    ] {
        server.expect(expected(
            "/api/v2/responses/resp_contract/events",
            None,
            wire.into_bytes(),
        ));
        let mut stream = server
            .client
            .responses_v2()
            .replay("resp_contract", None)
            .await
            .unwrap();
        loop {
            match stream.next().await.expect("malformed stream must fail") {
                Ok(_) => {}
                Err(Error::Protocol { .. }) => break,
                other => panic!("unexpected {other:?}"),
            }
        }
    }
}
#[tokio::test]
async fn dropping_observation_does_not_cancel_work_and_bad_inputs_never_reach_http() {
    let server = WireServer::new().await;
    let request = CreateResponseRequest::new("agt_fixture", "Hello");
    server.expect(expected(
        "/api/v2/responses",
        Some(serde_json::to_value(&request).unwrap()),
        fixture::wire(&events()[..1]),
    ));
    let stream = server
        .client
        .responses_v2()
        .create(&request, None)
        .await
        .unwrap();
    drop(stream);
    for key in ["", &"x".repeat(257), "line\nbreak"] {
        assert!(matches!(
            server
                .client
                .responses_v2()
                .create(&request, Some(key))
                .await,
            Err(Error::Validation { .. })
        ));
    }
    assert!(matches!(
        server
            .client
            .responses_v2()
            .create_for_agent("agt_other", &request, None)
            .await,
        Err(Error::Validation { .. })
    ));
    assert!(matches!(
        server
            .client
            .responses_v2()
            .replay("resp_contract", Some(u64::MAX))
            .await,
        Err(Error::Validation { .. })
    ));
    assert_eq!(server.request_count(), 1);
}
#[test]
fn nested_http_errors_preserve_code_parameter_request_identity_and_retry_after() {
    let error = Error::from_http_problem(503,Some("req_v2".into()),Some("1"),&serde_json::to_vec(&json!({"error":{"type":"server_error","code":"response_unavailable","message":"Retry the same key","param":null}})).unwrap());
    let Error::Api {
        status,
        code,
        request_id,
        retry_after,
        extensions,
        ..
    } = error
    else {
        panic!("expected API error");
    };
    assert_eq!(status, 503);
    assert_eq!(code.as_deref(), Some("response_unavailable"));
    assert_eq!(request_id.as_deref(), Some("req_v2"));
    assert_eq!(retry_after, Some(std::time::Duration::from_secs(1)));
    assert_eq!(extensions["type"], "server_error");
    assert_eq!(extensions["param"], Value::Null);
}

#[tokio::test]
async fn receipt_headers_and_preauthentication_refusals_work_under_a_proxy_prefix() {
    let server = WireServer::new().await;
    assert!(server.base_url.ends_with("/gateway"));
    for anonymous in [true, false] {
        let mut reply = expected("/api/v2/responses/resp_contract/events",None,serde_json::to_vec(&json!({"error":{"type":"invalid_request_error","code":"authentication_required","message":"Authenticate","param":null}})).unwrap());
        reply.status = 401;
        if anonymous {
            reply.headers.remove("authorization");
        }
        server.expect(reply);
        let client = if anonymous {
            &server.anonymous
        } else {
            &server.client
        };
        assert!(matches!(
            client.responses_v2().replay("resp_contract", None).await,
            Err(Error::Api { status: 401, .. })
        ));
    }
    for header in ["x-cma-response-id", "x-cma-command-id"] {
        let mut reply = expected(
            "/api/v2/responses/resp_contract/events",
            None,
            fixture::wire(&events()),
        );
        reply.response_headers.remove(header);
        server.expect(reply);
        assert!(matches!(
            server
                .client
                .responses_v2()
                .replay("resp_contract", None)
                .await,
            Err(Error::Protocol { .. })
        ));
    }
    let mut reply = expected(
        "/api/v2/responses/resp_contract/events",
        None,
        fixture::wire(&events()),
    );
    reply
        .response_headers
        .insert("x-cma-response-id".into(), "resp_foreign".into());
    server.expect(reply);
    assert!(matches!(
        server
            .client
            .responses_v2()
            .replay("resp_contract", None)
            .await,
        Err(Error::Protocol { .. })
    ));
    assert_eq!(server.request_count(), 5);
}

#[test]
fn legacy_rest_error_extension_named_error_remains_intact() {
    let error = Error::from_http_problem(409,None,None,&serde_json::to_vec(&json!({"status":409,"code":"legacy_conflict","detail":"Legacy detail","error":{"diagnostic":"retained"}})).unwrap());
    let Error::Api {
        code, extensions, ..
    } = error
    else {
        panic!("expected legacy error");
    };
    assert_eq!(code.as_deref(), Some("legacy_conflict"));
    assert_eq!(extensions["error"]["diagnostic"], "retained");
}
