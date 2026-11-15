use super::*;
use crate::telemetry::turn_trace::{reader::ProviderReader, testing};
use axum::http::HeaderMap;
use serde_json::{json, Value};
use std::{sync::mpsc::Receiver, time::Duration};

fn read(records: &Receiver<Vec<u8>>) -> Value {
    serde_json::from_slice(&records.recv_timeout(Duration::from_secs(2)).unwrap()).unwrap()
}

fn headers(request: Uuid, turn: Uuid) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("x-request-id", request.to_string().parse().unwrap());
    headers.insert("x-nyxid-trace-id", turn.to_string().parse().unwrap());
    headers
}

#[test]
fn queued_admission_links_server_ingress_after_headers_without_guessing_a_turn() {
    let (writer, records, _guard) = testing::capture();
    let mut reader = ProviderReader::new(true, true, None).unwrap();
    reader.source = Some(SourceRequest::start(writer));
    let request = Uuid::new_v4();
    let turn = Uuid::new_v4();
    reader.response(&headers(request, request), 200);
    assert!(reader.association().is_none());
    let start = read(&records);
    let end = read(&records);
    assert_eq!(start["span_id"], end["span_id"]);
    assert_eq!(start["request_id"], end["request_id"]);
    assert!(start.get("turn_id").is_none());
    assert!(end.get("turn_id").is_none());
    assert_eq!(end["outcome"], "completed");
    assert_eq!(end["ingress_request_id"], request.to_string());
    reader.snapshot(&json!({"id":"resp_private-capability-canary"}));
    let event = json!({"type":"response.nyxid_trace",
        "response_id":"resp_private-capability-canary", "nyxid_trace":{
        "version":1,"turn_id":turn.to_string(),"trace_id":turn.simple().to_string(),
        "parent_span_id":"0123456789abcdef","ingress_request_id":request.to_string()}});
    reader.event(&event);
    reader.event(&event);
    let link = read(&records);
    assert_eq!(link["span_id"], start["span_id"]);
    assert_eq!(link["phase"], "link");
    assert_eq!(link["related_turn_id"], turn.to_string());
    assert_eq!(link["ingress_request_id"], start["request_id"]);
    assert_eq!(link["turn_id"], turn.to_string());
    let remote = read(&records);
    assert_eq!(remote["ingress_request_id"], request.to_string());
    assert!(!format!("{start}{end}{link}{remote}").contains("private"));
    assert!(records.try_recv().is_err());
    if let Some(path) = std::env::var_os("CMAEG_TEST_TRACE_SOURCE_EXPORT") {
        let lines = [start, end, link, remote]
            .map(|value| value.to_string() + "\n")
            .concat();
        std::fs::write(path, lines).unwrap();
    }
}

#[test]
fn immediate_headers_link_once_and_missing_or_invalid_headers_remain_gaps() {
    let (writer, records, _guard) = testing::capture();
    let request = Uuid::new_v4();
    let turn = Uuid::new_v4();
    let mut reader = ProviderReader::new(true, true, None).unwrap();
    reader.source = Some(SourceRequest::start(writer.clone()));
    reader.response(&headers(request, turn), 200);
    let start = read(&records);
    let end = read(&records);
    let link = read(&records);
    assert_eq!(start["span_id"], end["span_id"]);
    assert_eq!(link["related_turn_id"], turn.to_string());
    assert_eq!(read(&records)["ingress_request_id"], request.to_string());
    let association = reader.association().unwrap();
    assert_eq!(association.turn_id, turn);
    assert!(association.parent_span_id.is_none());
    reader.response(&headers(Uuid::new_v4(), Uuid::new_v4()), 200);
    assert_eq!(reader.association(), Some(association));
    assert!(records.try_recv().is_err());
    for mut invalid in [HeaderMap::new(), headers(request, request)] {
        invalid.insert("x-nyxid-trace-id", "private-header-canary".parse().unwrap());
        let mut reader = ProviderReader::new(true, true, None).unwrap();
        reader.source = Some(SourceRequest::start(writer.clone()));
        reader.response(&invalid, 403);
        assert!(reader.association().is_none());
        let start = read(&records);
        let end = read(&records);
        assert_eq!(start["span_id"], end["span_id"]);
        assert_eq!(end["http_status"], 403);
        // A complete refusal HTTP exchange alone cannot confirm platform delivery.
        assert_eq!(end["outcome"], "completed");
        assert!(!format!("{start}{end}").contains("private"));
        assert!(records.try_recv().is_err());
    }
}

#[test]
fn cancelled_source_is_unknown_with_no_synthetic_admitted_turn() {
    let (writer, records, _guard) = testing::capture();
    drop(SourceRequest::start(writer));
    let start = read(&records);
    let end = read(&records);
    assert_eq!(start["span_id"], end["span_id"]);
    assert_eq!(start["request_id"], end["request_id"]);
    assert_eq!(end["outcome"], "unknown");
    assert!(end.get("turn_id").is_none());
    assert!(records.try_recv().is_err());
}
