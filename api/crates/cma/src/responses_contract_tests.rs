use serde_json::{json, Value};

use super::*;

fn sse_boundary(buffer: &[u8]) -> Option<(usize, usize)> {
    buffer
        .windows(2)
        .position(|window| window == b"\n\n")
        .map(|index| (index, 2))
        .or_else(|| {
            buffer
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|index| (index, 4))
        })
}

async fn next_responses_frame(body: &mut BodyDataStream, buffer: &mut Vec<u8>) -> (Value, String) {
    loop {
        if let Some((end, boundary_len)) = sse_boundary(buffer) {
            let bytes = buffer.drain(..end + boundary_len).collect::<Vec<_>>();
            let wire = String::from_utf8(bytes).expect("Responses SSE is UTF-8");
            if wire.lines().all(|line| {
                let line = line.trim_end_matches('\r');
                line.is_empty() || line.starts_with(':')
            }) {
                continue;
            }
            let event = wire
                .lines()
                .map(|line| line.trim_end_matches('\r'))
                .find_map(|line| line.strip_prefix("event:"))
                .map(str::trim)
                .expect("a Responses frame names its event");
            let data = wire
                .lines()
                .map(|line| line.trim_end_matches('\r'))
                .filter_map(|line| line.strip_prefix("data:"))
                .map(str::trim_start)
                .collect::<Vec<_>>()
                .join("\n");
            assert!(!data.is_empty(), "a Responses frame carries data: {wire:?}");
            let value: Value = serde_json::from_str(&data)
                .unwrap_or_else(|error| panic!("Responses data is JSON ({error}): {wire:?}"));
            assert_eq!(value["type"], event, "SSE event and payload type diverged");
            return (value, wire);
        }

        let bytes = tokio::time::timeout(Duration::from_secs(2), body.next())
            .await
            .expect("the Responses stream emits within the test deadline")
            .expect("the Responses stream remains open")
            .expect("the Responses body is readable");
        buffer.extend_from_slice(&bytes);
    }
}

async fn create_chat_conversation(surface: &TestSurface) -> (SessionId, Value) {
    let response = surface
        .app
        .clone()
        .oneshot(json_request(Method::POST, "/api/v1/conversations", "{}"))
        .await
        .expect("the composed router answers");
    if response.status() != StatusCode::OK {
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("the creation refusal is bounded");
        panic!(
            "conversation creation returned {status}: {}",
            String::from_utf8_lossy(&bytes)
        );
    }
    let body = assert_json(response, StatusCode::OK).await;
    assert_eq!(body["object"], "conversation");
    let id = SessionId(
        body["id"]
            .as_str()
            .expect("a conversation has an id")
            .to_string(),
    );
    wait_until_ready(&surface.sessions, &surface.caller, &id).await;
    (id, body)
}

#[tokio::test(flavor = "current_thread")]
async fn conversation_creation_survives_a_dropped_request() {
    let surface = chat_test_surface(true);
    let pause = surface.kernel.pause_next_provision().await;
    let request_app = surface.app.clone();
    let request = tokio::spawn(async move {
        request_app
            .oneshot(json_request(Method::POST, "/api/v1/conversations", "{}"))
            .await
            .expect("the conversation route answers")
    });

    pause.wait_until_reached().await;
    request.abort();
    assert!(request
        .await
        .expect_err("the simulated browser request was dropped")
        .is_cancelled());
    pause.release();

    let id = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let candidate = surface
                .chat_hosts
                .get_for_owner(&surface.caller.subject)
                .await
                .expect("the chat host store remains readable")
                .filter(|host| host.status == ChatHostStatus::Active)
                .and_then(|host| {
                    host.threads
                        .iter()
                        .find(|slot| !slot.removed)
                        .map(|slot| slot.session_id.clone())
                });
            let durable = match candidate {
                Some(id)
                    if surface
                        .store
                        .get(&id)
                        .await
                        .expect("the session store remains readable")
                        .is_some() =>
                {
                    Some(id)
                }
                _ => None,
            };
            if let Some(id) = durable {
                break id;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("server-owned conversation creation completes after the request is dropped");

    let host = surface
        .chat_hosts
        .get_for_owner(&surface.caller.subject)
        .await
        .unwrap()
        .expect("the active chat host remains durable");
    assert_eq!(host.threads.iter().filter(|slot| !slot.removed).count(), 1);
    assert_eq!(host.threads[0].session_id, id);
    let session = surface
        .sessions
        .get_session(&surface.caller, &id)
        .await
        .expect("the detached creation commits a readable conversation");
    assert_eq!(session.status, SessionStatus::Ready);
    assert!(session.chat.is_some());
}

async fn retrieve_response(app: &Router, id: &str) -> Value {
    let response = app
        .clone()
        .oneshot(
            Request::get(format!("/api/v1/responses/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("the composed router answers");
    assert_json(response, StatusCode::OK).await
}

async fn wait_for_completed_response(app: &Router, id: &str) -> Value {
    for _ in 0..200 {
        let response = retrieve_response(app, id).await;
        if response["status"] == "completed" {
            return response;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("response {id} did not become durable and completed");
}

#[tokio::test]
async fn responses_full_turn_is_streamed_retrieved_paginated_and_not_body_owned() {
    let surface = chat_test_surface(true);
    let (id, conversation) = create_chat_conversation(&surface).await;
    assert_eq!(conversation["metadata"], json!({}));

    let retrieved = surface
        .app
        .clone()
        .oneshot(
            Request::get(format!("/api/v1/conversations/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("the composed router answers");
    assert_eq!(assert_json(retrieved, StatusCode::OK).await, conversation);

    let empty = surface
        .app
        .clone()
        .oneshot(
            Request::get(format!("/api/v1/conversations/{id}/items"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("the composed router answers");
    let empty = assert_json(empty, StatusCode::OK).await;
    assert_eq!(
        empty,
        json!({
            "object": "list",
            "data": [],
            "first_id": null,
            "last_id": null,
            "has_more": false
        })
    );

    let response = surface
        .app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/responses",
            json!({
                "conversation": id.as_str(),
                "input": "First prompt",
                "stream": true,
                "metadata": { "trace": "full-turn" },
                "model": "caller-model-is-ignored"
            })
            .to_string(),
        ))
        .await
        .expect("the composed router answers");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE),
        Some(&HeaderValue::from_static("text/event-stream"))
    );
    let mut body = response.into_body().into_data_stream();
    let mut buffer = Vec::new();

    surface.kernel.emit_reasoning_chunk("Considering").await;
    let mut events = Vec::new();
    loop {
        let (event, _) = next_responses_frame(&mut body, &mut buffer).await;
        let reasoning_arrived = event["type"] == "response.reasoning_summary_text.delta";
        events.push(event);
        if reasoning_arrived {
            break;
        }
    }
    surface.kernel.emit_message_chunk("Complete answer").await;
    loop {
        let (event, _) = next_responses_frame(&mut body, &mut buffer).await;
        let text_arrived = event["type"] == "response.output_text.delta";
        events.push(event);
        if text_arrived {
            break;
        }
    }
    surface.kernel.emit_turn_ended_with_usage(12, 3).await;
    loop {
        let (event, _) = next_responses_frame(&mut body, &mut buffer).await;
        let terminal = matches!(
            event["type"].as_str(),
            Some("response.completed" | "response.failed" | "response.incomplete")
        );
        events.push(event);
        if terminal {
            break;
        }
    }

    for (expected, event) in events.iter().enumerate() {
        assert_eq!(event["sequence_number"], expected as u64);
    }
    assert_eq!(events[0]["type"], "response.created");
    assert_eq!(events[1]["type"], "response.in_progress");
    assert!(events
        .iter()
        .any(|event| event["type"] == "response.reasoning_summary_part.added"));
    assert!(events
        .iter()
        .any(|event| event["type"] == "response.output_item.added"));
    let terminal = events.last().expect("the stream has a terminal event");
    assert_eq!(terminal["type"], "response.completed");
    assert_eq!(terminal["response"]["model"], "chat-model-fixture");
    assert_eq!(terminal["response"]["metadata"]["trace"], "full-turn");
    assert_eq!(terminal["response"]["usage"]["input_tokens"], 12);
    assert_eq!(terminal["response"]["usage"]["output_tokens"], 3);
    assert_eq!(terminal["response"]["usage"]["total_tokens"], 15);
    assert_eq!(
        terminal["response"]["output"][0]["content"][0]["text"],
        "Complete answer"
    );
    assert_eq!(
        terminal["response"]["reasoning"],
        json!({
            "effort": null,
            "summary": [{
                "type": "summary_text",
                "text": "Considering"
            }]
        })
    );
    let first_response_id = terminal["response"]["id"]
        .as_str()
        .expect("a response has an id")
        .to_string();

    let durable = retrieve_response(&surface.app, &first_response_id).await;
    assert_eq!(durable, terminal["response"]);
    assert_eq!(durable["reasoning"]["summary"][0]["text"], "Considering");

    let items = surface
        .app
        .clone()
        .oneshot(
            Request::get(format!("/api/v1/conversations/{id}/items"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("the composed router answers");
    let items = assert_json(items, StatusCode::OK).await;
    assert_eq!(items["data"].as_array().unwrap().len(), 3);
    assert_eq!(items["data"][0]["role"], "assistant");
    assert_eq!(items["data"][0]["content"][0]["text"], "Complete answer");
    assert_eq!(items["data"][1]["type"], "cma_chat.working_notes");
    assert_eq!(items["data"][1]["response_id"], first_response_id);
    assert_eq!(items["data"][1]["reasoning"], "Considering");
    assert_eq!(items["data"][1]["messages"], json!([]));
    assert_eq!(items["data"][2]["role"], "user");
    assert_eq!(items["data"][2]["content"][0]["text"], "First prompt");

    let first_page = surface
        .app
        .clone()
        .oneshot(
            Request::get(format!("/api/v1/conversations/{id}/items?limit=1"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("the composed router answers");
    let first_page = assert_json(first_page, StatusCode::OK).await;
    assert_eq!(first_page["data"].as_array().unwrap().len(), 1);
    assert_eq!(first_page["has_more"], true);
    let after = first_page["last_id"].as_str().unwrap();
    let second_page = surface
        .app
        .clone()
        .oneshot(
            Request::get(format!(
                "/api/v1/conversations/{id}/items?limit=1&after={after}"
            ))
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .expect("the composed router answers");
    let second_page = assert_json(second_page, StatusCode::OK).await;
    assert_eq!(second_page["data"][0]["type"], "cma_chat.working_notes");
    assert_eq!(second_page["has_more"], true);
    let after = second_page["last_id"].as_str().unwrap();
    let third_page = surface
        .app
        .clone()
        .oneshot(
            Request::get(format!(
                "/api/v1/conversations/{id}/items?limit=1&after={after}"
            ))
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .expect("the composed router answers");
    let third_page = assert_json(third_page, StatusCode::OK).await;
    assert_eq!(third_page["data"][0]["role"], "user");
    assert_eq!(third_page["data"][0]["content"][0]["text"], "First prompt");
    assert_eq!(third_page["has_more"], false);

    let dropped = surface
        .app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/responses",
            json!({
                "conversation": id.as_str(),
                "input": "Continue after disconnect",
                "stream": true,
                "previous_response_id": first_response_id
            })
            .to_string(),
        ))
        .await
        .expect("the composed router answers");
    assert_eq!(dropped.status(), StatusCode::OK);
    drop(dropped);

    let durable_events = surface
        .sessions
        .events_since(&surface.caller, &id, None)
        .await
        .expect("the conversation log is readable");
    let second_turn = durable_events
        .iter()
        .rev()
        .find(|event| matches!(event.kind, EventKind::TurnStarted))
        .expect("the dropped stream still admitted a turn")
        .seq;
    let second_response_id = format!("resp_{}_{}", id.as_str(), second_turn);
    surface.kernel.emit_message_chunk("Still completed").await;
    surface.kernel.emit_turn_ended_with_usage(7, 2).await;
    let second = wait_for_completed_response(&surface.app, &second_response_id).await;
    assert_eq!(second["output"][0]["content"][0]["text"], "Still completed");
    assert_eq!(second["previous_response_id"], first_response_id);

    let deliveries = surface.kernel.conversation_inputs.load(Ordering::SeqCst);
    let stale = surface
        .app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/responses",
            json!({
                "conversation": id.as_str(),
                "input": "Fork the thread",
                "stream": true,
                "previous_response_id": first_response_id
            })
            .to_string(),
        ))
        .await
        .expect("the composed router answers");
    let stale = assert_problem(stale, StatusCode::CONFLICT).await;
    assert_eq!(stale["code"], "previous_response_id_conflict");
    assert_eq!(stale["field"], "previous_response_id");
    assert_eq!(
        surface.kernel.conversation_inputs.load(Ordering::SeqCst),
        deliveries,
        "a stale predecessor never reaches the runtime"
    );
}

#[tokio::test]
async fn response_post_revives_a_paused_chat_thread_before_streaming_the_turn() {
    let surface = chat_test_surface(true);
    let (id, _) = create_chat_conversation(&surface).await;
    let host = surface
        .chat_hosts
        .get_for_owner(&surface.caller.subject)
        .await
        .expect("the host store is readable")
        .expect("conversation creation allocated a shared host");
    assert_eq!(host.status, ChatHostStatus::Active);
    assert!(host.runtime_id.is_some());
    assert!(host.last_transition_claim.is_none());
    let candidates = surface
        .sessions
        .chat_pause_candidates(
            "2099-09-01T00:00:00Z",
            PageRequest::new(MAX_PAGE_SIZE, None),
        )
        .await
        .expect("pause candidates are readable");
    assert_eq!(
        candidates
            .items
            .iter()
            .map(|candidate| &candidate.id)
            .collect::<Vec<_>>(),
        [&host.id],
        "the composed thread must be durably quiet before the pause"
    );
    let pause = surface
        .sessions
        .pause_chat_host(&host.id, "2099-09-01T00:00:00Z")
        .await
        .expect("the inactive host can be paused");
    let after_pause = surface.chat_hosts.get(&host.id).await.unwrap().unwrap();
    assert!(
        matches!(pause, layer2_core::ChatPauseOutcome::Complete(_)),
        "unexpected pause outcome: {pause:?}; note: {:?}",
        after_pause.last_operator_note
    );
    let paused = surface
        .chat_hosts
        .get(&host.id)
        .await
        .unwrap()
        .expect("the paused host remains durable");
    assert_eq!(paused.status, ChatHostStatus::Paused);
    assert_eq!(
        surface
            .sessions
            .get_session(&surface.caller, &id)
            .await
            .unwrap()
            .status,
        SessionStatus::Idle
    );

    let response = surface
        .app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/responses",
            json!({
                "conversation": id.as_str(),
                "input": "Revive this thread",
                "stream": true
            })
            .to_string(),
        ))
        .await
        .expect("the composed router answers after restoration");
    assert_eq!(response.status(), StatusCode::OK);
    let active = surface
        .chat_hosts
        .get(&host.id)
        .await
        .unwrap()
        .expect("the revived host remains durable");
    assert_eq!(active.status, ChatHostStatus::Active);
    assert_eq!(active.runtime_generation, 2);

    let mut body = response.into_body().into_data_stream();
    let mut buffer = Vec::new();
    let (created, _) = next_responses_frame(&mut body, &mut buffer).await;
    let (in_progress, _) = next_responses_frame(&mut body, &mut buffer).await;
    assert_eq!(created["type"], "response.created");
    assert_eq!(in_progress["type"], "response.in_progress");
    assert_eq!(created["response"]["previous_response_id"], Value::Null);

    surface.kernel.emit_message_chunk("Restored answer").await;
    surface.kernel.emit_turn_ended_with_usage(5, 2).await;
    let mut terminal = None;
    while terminal.is_none() {
        let (event, _) = next_responses_frame(&mut body, &mut buffer).await;
        if event["type"] == "response.completed" {
            terminal = Some(event);
        }
    }
    let terminal = terminal.unwrap();
    assert_eq!(
        terminal["response"]["output"][0]["content"][0]["text"],
        "Restored answer"
    );
    assert_eq!(terminal["response"]["usage"]["total_tokens"], 7);
}

#[tokio::test]
async fn responses_refusals_are_named_authenticated_and_bounded() {
    let authenticated = test_surface(true).app;
    for (method, path, field) in [
        (
            Method::DELETE,
            "/api/v1/responses/resp_sesn_missing_1",
            "delete",
        ),
        (
            Method::POST,
            "/api/v1/responses/resp_sesn_missing_1/cancel",
            "background",
        ),
    ] {
        let response = authenticated
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("the composed router answers");
        let body = assert_problem(response, StatusCode::UNPROCESSABLE_ENTITY).await;
        assert_eq!(body["code"], "responses_field_unsupported");
        assert_eq!(body["field"], field);
    }

    let anonymous = test_surface(false).app;
    for request in [
        Request::delete("/api/v1/responses/resp_sesn_missing_1")
            .body(Body::empty())
            .unwrap(),
        Request::post("/api/v1/responses/resp_sesn_missing_1/cancel")
            .body(Body::empty())
            .unwrap(),
    ] {
        let response = anonymous
            .clone()
            .oneshot(request)
            .await
            .expect("the composed router answers");
        let body = assert_problem(response, StatusCode::UNAUTHORIZED).await;
        assert_eq!(body["code"], "not_authenticated");
    }

    for (query, field) in [
        ("stream=true", "stream"),
        ("starting_after=1", "starting_after"),
        ("include=message.output_text.logprobs", "include"),
    ] {
        let response = authenticated
            .clone()
            .oneshot(
                Request::get(format!("/api/v1/responses/resp_sesn_missing_1?{query}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("the composed router answers");
        let body = assert_problem(response, StatusCode::UNPROCESSABLE_ENTITY).await;
        assert_eq!(body["field"], field, "query {query}");
    }

    for limit in ["0", "101"] {
        let response = authenticated
            .clone()
            .oneshot(
                Request::get(format!(
                    "/api/v1/conversations/sesn_missing/items?limit={limit}"
                ))
                .body(Body::empty())
                .unwrap(),
            )
            .await
            .expect("the composed router answers");
        let body = assert_problem(response, StatusCode::UNPROCESSABLE_ENTITY).await;
        assert_eq!(body["field"], "limit");
    }
    let malformed = authenticated
        .oneshot(
            Request::get("/api/v1/conversations/sesn_missing/items?limit=not-a-number")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("the composed router answers");
    let malformed = assert_problem(malformed, StatusCode::BAD_REQUEST).await;
    assert_eq!(malformed["code"], "invalid_query");
}

#[tokio::test]
async fn responses_conceal_foreign_non_chat_and_invalid_cursors() {
    let surface = chat_test_surface(true);
    let (owned_chat, _) = create_chat_conversation(&surface).await;

    let bad_cursor = surface
        .app
        .clone()
        .oneshot(
            Request::get(format!(
                "/api/v1/conversations/{owned_chat}/items?after=msg_not_in_this_thread"
            ))
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .expect("the composed router answers");
    let bad_cursor = assert_problem(bad_cursor, StatusCode::UNPROCESSABLE_ENTITY).await;
    assert_eq!(bad_cursor["field"], "after");

    let non_chat = create_ready_conversation_session(&surface.sessions, &surface.caller).await;
    for path in [
        format!("/api/v1/conversations/{non_chat}"),
        format!("/api/v1/conversations/{non_chat}/items"),
        format!("/api/v1/responses/resp_{non_chat}_1"),
    ] {
        let response = surface
            .app
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .expect("the composed router answers");
        let body = assert_problem(response, StatusCode::NOT_FOUND).await;
        assert_eq!(body["code"], "session_not_found");
    }

    let foreign_caller = Caller::new(
        SubjectId("responses-foreign-owner".into()),
        surface.caller.authorization.clone(),
    );
    let foreign = surface
        .sessions
        .create_session(
            &foreign_caller,
            SessionSpec {
                surface: Some(layer2_core::SessionSurface::Chat),
                options: Default::default(),
                geometry: None,
                environment_profile: None,
                caller_credential: Some(TEST_DELEGATED_CREDENTIAL.into()),
                runtime_credential: None,
                requested_mode: None,
                quoted_rate_card: None,
            },
        )
        .await
        .expect("the foreign chat conversation is accepted");
    wait_until_ready(&surface.sessions, &foreign_caller, &foreign.id).await;
    for path in [
        format!("/api/v1/conversations/{}", foreign.id),
        format!("/api/v1/conversations/{}/items", foreign.id),
        format!("/api/v1/responses/resp_{}_1", foreign.id),
    ] {
        let response = surface
            .app
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .expect("the composed router answers");
        let body = assert_problem(response, StatusCode::NOT_FOUND).await;
        assert_eq!(body["code"], "session_not_found");
    }
    let foreign_post = surface
        .app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/responses",
            json!({
                "conversation": foreign.id.as_str(),
                "input": "must remain concealed",
                "stream": true
            })
            .to_string(),
        ))
        .await
        .expect("the composed router answers");
    let foreign_post = assert_problem(foreign_post, StatusCode::NOT_FOUND).await;
    assert_eq!(foreign_post["code"], "session_not_found");
}
