use super::*;
use crate::telemetry::turn_trace::ProviderReader;
use serde_json::json;

fn frame(value: Value) -> SseFrame {
    SseFrame {
        event: None,
        data: value.to_string(),
        id: None,
        retry: None,
    }
}

fn association() -> Value {
    json!({"version":1,"turn_id":"11111111-1111-4111-8111-111111111111",
        "trace_id":"11111111111141118111111111111111","parent_span_id":"0123456789abcdef",
        "ingress_request_id":"22222222-2222-4222-8222-222222222222"})
}

#[test]
fn negotiated_parser_consumes_event_without_exposing_metadata_as_model_output() {
    let event = frame(
        json!({"type":"response.nyxid_trace","response_id":"resp_synthetic","nyxid_trace":association()}),
    );
    let mut trace = ProviderReader::new(true, true, Some("resp_synthetic")).unwrap();
    assert_eq!(
        parse_event_with_trace(&event, Some(&mut trace)),
        Ok(ResponsesEvent::NyxidTrace)
    );
    assert_eq!(
        trace.turn_id().unwrap().to_string(),
        "11111111-1111-4111-8111-111111111111"
    );
    assert_eq!(
        parse_event(&event),
        Ok(ResponsesEvent::Unknown {
            r#type: "response.nyxid_trace".into()
        })
    );
    assert_eq!(
        parse_event_with_trace(&event, ProviderReader::new(false, true, None).as_mut()),
        parse_event(&event)
    );
}

#[test]
fn snapshot_association_and_conflicting_metadata_leave_terminal_output_intact() {
    let mut trace = ProviderReader::new(true, true, None).unwrap();
    let created = frame(
        json!({"type":"response.created","response":{"id":"resp_synthetic","nyxid_trace":association()}}),
    );
    assert_eq!(
        parse_event_with_trace(&created, Some(&mut trace)),
        parse_event(&created)
    );
    let original = trace.association();
    assert!(original.is_some());
    let mut other = association();
    other["parent_span_id"] = json!("fedcba9876543210");
    for value in [other, json!({"private-canary":"private-content"})] {
        let completed = frame(json!({"type":"response.completed","response":{
            "id":"resp_synthetic","nyxid_trace":value,
            "output":[{"type":"message","content":[{"type":"output_text","text":"answer"}]}]
        }}));
        assert_eq!(
            parse_event_with_trace(&completed, Some(&mut trace)),
            parse_event(&completed)
        );
        assert_eq!(trace.association(), original);
    }
}
