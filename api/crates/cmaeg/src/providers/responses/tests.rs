use super::*;

fn frame(data: &str) -> SseFrame {
    SseFrame {
        event: None,
        data: data.into(),
        id: None,
        retry: None,
    }
}

#[test]
fn parses_known_events_and_forwards_unknown_types() {
    assert_eq!(
        parse_event(&frame(
            r#"{"type":"response.created","response":{"id":"resp_1"}}"#
        )),
        Ok(ResponsesEvent::Created {
            response_id: "resp_1".into(),
            native_agent_id: None,
            thread_reply: false
        })
    );
    assert_eq!(
        parse_event(&frame(r#"{"type":"response.future"}"#)),
        Ok(ResponsesEvent::Unknown {
            r#type: "response.future".into()
        })
    );
}

#[test]
fn native_agent_identity_is_optional_bounded_provider_evidence() {
    for (value, expected) in [
        (
            serde_json::json!("agt_native_provider"),
            Some("agt_native_provider"),
        ),
        (Value::Null, None),
        (serde_json::json!(""), None),
        (serde_json::json!(" "), None),
        (serde_json::json!("x".repeat(257)), None),
        (serde_json::json!("agent\nheader"), None),
        (serde_json::json!(42), None),
    ] {
        let raw = serde_json::json!({"type":"response.created","response":{
            "id":"resp_native","conversation":{"id":value}}});
        let ResponsesEvent::Created {
            native_agent_id, ..
        } = parse_event(&frame(&raw.to_string())).unwrap()
        else {
            panic!("created")
        };
        assert_eq!(native_agent_id.as_deref(), expected);
    }
}

#[test]
fn native_thread_choice_is_opt_in_on_provider_acceptance() {
    assert!(parse_event(&frame(
        r#"{"type":"response.created","response":{"id":"resp_thread","thread_reply":"true"}}"#
    ))
    .is_err());
    assert_eq!(
        parse_event(&frame(
            r#"{"type":"response.created","response":{"id":"resp_thread","thread_reply":true}}"#
        )),
        Ok(ResponsesEvent::Created {
            response_id: "resp_thread".into(),
            native_agent_id: None,
            thread_reply: true
        })
    );
}

#[test]
fn parses_output_items_and_terminal_errors() {
    let event = parse_event(&frame(
        r#"{"type":"response.output_item.added","output_index":2,"item":{"type":"mcp_call","id":"m1","name":"tools","status":"in_progress"}}"#,
    ))
    .unwrap();
    assert_eq!(
        event,
        ResponsesEvent::OutputItemAdded {
            index: 2,
            item: OutputItem::McpCall {
                id: "m1".into(),
                name: "tools".into(),
                status: "in_progress".into(),
            }
        }
    );
    assert!(matches!(
        parse_event(&frame(
            r#"{"type":"response.failed","response":{"error":{"code":"bad","message":"no"}}}"#
        )),
        Ok(ResponsesEvent::Failed { .. })
    ));
}
