use std::collections::BTreeMap;
use std::time::Duration;

use axum::body::{to_bytes, Body};
use axum::http::{HeaderMap, Request};
use futures_util::StreamExt;
use layer2_core::SubjectId;
use serde_json::{json, Value};
use tower::ServiceExt;

use super::{BodyKind, Case, RequestFixture, Scenario, Symbols};
use crate::AuthenticatedSubject;

const MAX_RAW_BODY_REPEAT: usize = 2 * 1024 * 1024 + 1;
const MAX_RAW_BODY_BYTES: usize = 8 * 1024 * 1024;

pub(super) async fn run(scenario: &Scenario, symbols: &mut Symbols, case: &mut Case, record: bool) {
    if record {
        eprintln!("recording {}", case.id);
    }
    let path = symbols.resolve_text(&case.request.path);
    let mut request = Request::builder()
        .method(case.request.method.as_str())
        .uri(path);
    for (name, value) in &case.request.headers {
        request = request.header(name.as_str(), symbols.resolve_text(value));
    }
    let request_body = Body::from(encode_request_body(&case.request, symbols));
    let mut request = request.body(request_body).unwrap();
    if let Some(subject) = scenario.subject {
        request
            .extensions_mut()
            .insert(AuthenticatedSubject(SubjectId::new(subject)));
        request
            .extensions_mut()
            .insert(scenario.authorization.clone());
    }
    let response = response_timeout(&case.scenario, async {
        if case.operation == "GET /api/v1/agents/{id}/desktop/ws" {
            Ok(Box::pin(super::desktop_cases::upgrade_response(scenario, request)).await)
        } else {
            scenario.router.clone().oneshot(request).await
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{} response timed out", case.id))
    .unwrap();
    assert_eq!(
        response.status().as_u16(),
        case.response.status,
        "{} status",
        case.id
    );
    let headers = response_headers(response.headers());
    let body = match case.response.body_kind {
        BodyKind::Json => {
            let bytes = to_bytes(response.into_body(), 8 * 1024 * 1024)
                .await
                .unwrap();
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|error| panic!("{} invalid JSON response: {error}", case.id))
        }
        BodyKind::Empty => {
            assert!(
                to_bytes(response.into_body(), 8 * 1024 * 1024)
                    .await
                    .unwrap()
                    .is_empty(),
                "{} expected empty body",
                case.id
            );
            Value::Null
        }
        BodyKind::Text => {
            let bytes = to_bytes(response.into_body(), 8 * 1024 * 1024)
                .await
                .unwrap();
            Value::String(String::from_utf8(bytes.to_vec()).expect("text response is UTF-8"))
        }
        BodyKind::Sse => {
            assert_eq!(
                headers.get("content-type").map(String::as_str),
                Some("text/event-stream"),
                "{} SSE content type",
                case.id
            );
            let frames = case
                .response
                .body
                .as_array()
                .expect("SSE fixture body must be an array")
                .len();
            assert!(frames > 0, "SSE fixtures must prove at least one frame");
            read_sse(response.into_body(), frames, case.response.stream_prefix).await
        }
    };
    if let Some(expected) = case.response.body["code"]
        .as_str()
        .or_else(|| case.response.body["error"]["code"].as_str())
    {
        let actual = body["code"]
            .as_str()
            .or_else(|| body["error"]["code"].as_str());
        assert_eq!(actual, Some(expected), "{} named problem", case.id);
    }
    for (symbol, pointer) in &case.captures {
        assert!(
            pointer.starts_with('/'),
            "{} invalid JSON capture pointer",
            case.id
        );
        let actual = body
            .pointer(pointer)
            .unwrap_or_else(|| panic!("{} missing actual capture {pointer}", case.id));
        let canonical = case
            .response
            .body
            .pointer(pointer)
            .unwrap_or_else(|| panic!("{} missing canonical capture {pointer}", case.id));
        symbols.capture(symbol, actual, canonical);
    }
    for (symbol, header) in &case.header_captures {
        let actual = Value::String(
            headers
                .get(header)
                .unwrap_or_else(|| panic!("{} missing receipt header {header}", case.id))
                .clone(),
        );
        let canonical = Value::String(
            case.response
                .headers
                .get(header)
                .expect("canonical receipt header")
                .clone(),
        );
        symbols.capture(symbol, &actual, &canonical);
    }
    let headers: BTreeMap<String, String> =
        serde_json::from_value(symbols.normalize(symbols.response_tokens(
            serde_json::to_value(headers).unwrap(),
            &serde_json::to_value(&case.response.headers).unwrap(),
        )))
        .unwrap();
    let normalized = symbols.normalize(symbols.response_tokens(body, &case.response.body));
    if record {
        case.response.headers = headers;
        case.response.body = normalized;
    } else {
        assert_eq!(
            headers, case.response.headers,
            "{} response headers (excluding derived content-length)",
            case.id
        );
        assert_eq!(normalized, case.response.body, "{} response body", case.id);
    }
}

fn response_headers(headers: &HeaderMap) -> BTreeMap<String, String> {
    let mut values = BTreeMap::new();
    for (name, value) in headers {
        assert!(
            values
                .insert(name.as_str().to_owned(), value.to_str().unwrap().to_owned())
                .is_none(),
            "single-value header fixtures cannot represent repeated {name}"
        );
    }
    values.remove("content-length");
    values
}

#[test]
fn repeated_response_headers_cannot_hide_earlier_values() {
    for name in ["set-cookie", "cache-control"] {
        let mut headers = HeaderMap::new();
        headers.append(name, "unexpected=earlier".parse().unwrap());
        headers.append(name, "expected=last".parse().unwrap());
        assert!(std::panic::catch_unwind(|| response_headers(&headers)).is_err());
    }
}

#[test]
fn single_value_response_headers_keep_exact_values_except_derived_length() {
    let mut headers = HeaderMap::new();
    headers.insert("set-cookie", "expected=last".parse().unwrap());
    headers.insert("content-length", "0".parse().unwrap());
    assert_eq!(
        response_headers(&headers),
        BTreeMap::from([("set-cookie".into(), "expected=last".into())])
    );
}

async fn response_timeout<F: std::future::Future>(
    scenario: &str,
    future: F,
) -> Result<F::Output, tokio::time::error::Elapsed> {
    let virtual_time = matches!(
        scenario,
        "experience_timeout_input"
            | "experience_timeout_context"
            | "experience_working"
            | "experience_no_steer"
            | "experience_pending"
            | "experience_checkpoint"
            | "experience_owner"
            | "experience_agui_ambiguous"
            | "experience_responses_ambiguous"
    );
    // Provisioning and queued legacy binding observation have bounded deadlines.
    let seconds = if virtual_time {
        tokio::time::pause();
        610
    } else {
        5
    };
    let result = tokio::time::timeout(Duration::from_secs(seconds), future).await;
    if virtual_time {
        tokio::time::resume();
    }
    result
}

fn encode_request_body(request: &RequestFixture, symbols: &Symbols) -> Vec<u8> {
    assert!(
        request.body.is_none() || request.raw_body.is_none(),
        "body and rawBody are mutually exclusive"
    );
    assert!(
        request.raw_body.is_some() || request.raw_body_repeat.is_none(),
        "rawBodyRepeat requires rawBody"
    );
    if let Some(raw) = &request.raw_body {
        let count = request.raw_body_repeat.unwrap_or(1);
        assert!(
            (1..=MAX_RAW_BODY_REPEAT).contains(&count),
            "rawBodyRepeat is out of range"
        );
        let raw = symbols.resolve_text(raw);
        let bytes = raw
            .len()
            .checked_mul(count)
            .expect("rawBody byte length overflow");
        assert!(
            bytes <= MAX_RAW_BODY_BYTES,
            "rawBody exceeds expanded byte budget"
        );
        raw.as_bytes().repeat(count)
    } else {
        request.body.as_ref().map_or_else(Vec::new, |body| {
            serde_json::to_vec(&symbols.resolve(body)).unwrap()
        })
    }
}

#[tokio::test]
async fn provisioning_timeout_cases_advance_virtual_time_then_restore_the_clock() {
    for scenario in ["experience_timeout_input", "experience_timeout_context"] {
        let start = tokio::time::Instant::now();
        response_timeout(scenario, async {
            tokio::time::sleep(Duration::from_secs(600)).await;
        })
        .await
        .unwrap();
        assert!(start.elapsed() >= Duration::from_secs(600));
        tokio::time::pause();
        tokio::time::resume();
    }
}

#[tokio::test]
async fn provisioning_timeout_budget_expiry_restores_the_clock() {
    let start = tokio::time::Instant::now();
    assert!(
        response_timeout("experience_timeout_context", std::future::pending::<()>())
            .await
            .is_err()
    );
    assert!(start.elapsed() >= Duration::from_secs(610));
    tokio::time::pause();
    tokio::time::resume();
}

#[tokio::test(start_paused = true)]
async fn ordinary_cases_keep_the_five_second_deadline() {
    let start = tokio::time::Instant::now();
    assert!(response_timeout("experience_timeout_context_other", async {
        tokio::time::sleep(Duration::from_secs(6)).await;
    })
    .await
    .is_err());
    assert_eq!(start.elapsed(), Duration::from_secs(5));
}

async fn read_sse(body: Body, expected_frames: usize, prefix: bool) -> Value {
    let mut stream = body.into_data_stream();
    let mut bytes = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(chunk) = stream.next().await {
            bytes.extend_from_slice(&chunk.unwrap());
            assert!(
                bytes.len() <= 8 * 1024 * 1024,
                "SSE fixture exceeded byte budget"
            );
            if prefix {
                if let Ok(text) = std::str::from_utf8(&bytes) {
                    let frames = parse_sse(text);
                    if frames.len() >= expected_frames {
                        return Value::Array(frames.into_iter().take(expected_frames).collect());
                    }
                }
            }
        }
        assert!(
            !prefix,
            "a declared open-stream prefix unexpectedly reached EOF"
        );
        let text = std::str::from_utf8(&bytes).expect("SSE response is UTF-8");
        assert!(
            text.replace("\r\n", "\n").ends_with("\n\n"),
            "incomplete final SSE frame"
        );
        let frames = parse_sse(text);
        assert_eq!(
            frames.len(),
            expected_frames,
            "SSE frame count differs at EOF"
        );
        Value::Array(frames)
    })
    .await
    .expect("SSE fixture timed out")
}

fn parse_sse(text: &str) -> Vec<Value> {
    let text = text.replace("\r\n", "\n");
    let mut frames = Vec::new();
    let complete = text
        .rsplit_once("\n\n")
        .map_or("", |(complete, _)| complete);
    for block in complete.split("\n\n") {
        let mut frame = serde_json::Map::new();
        let mut data = Vec::new();
        for line in block.lines() {
            if line.starts_with(':') {
                continue;
            }
            let (name, value) = line.split_once(':').unwrap_or((line, ""));
            let value = value.strip_prefix(' ').unwrap_or(value);
            match name {
                "data" => data.push(value),
                "event" | "id" => {
                    frame.insert(name.into(), json!(value));
                }
                "retry" => {
                    frame.insert(
                        name.into(),
                        json!(value.parse::<u64>().expect("SSE retry is numeric")),
                    );
                }
                _ => panic!("unexpected SSE field {name}"),
            }
        }
        if data.is_empty() {
            continue;
        }
        let data = data.join("\n");
        frame.insert(
            "data".into(),
            serde_json::from_str(&data).unwrap_or_else(|_| json!(data)),
        );
        frames.push(Value::Object(frame));
    }
    frames
}

#[test]
fn sse_parser_preserves_event_ids_data_and_retry_metadata() {
    assert_eq!(
        parse_sse(": keepalive\n\nevent: changed\nid: 12\nretry: 50\ndata: {\"value\":1}\n\n"),
        vec![json!({"event":"changed", "id":"12", "retry":50, "data":{"value":1}})]
    );
}

#[test]
fn partial_sse_frames_are_not_accepted_before_the_dispatch_boundary() {
    assert!(parse_sse("event: pending\nid: 1\ndata: {}\n").is_empty());
}

#[tokio::test]
#[should_panic(expected = "SSE frame count differs at EOF")]
async fn split_trailing_sse_frames_are_rejected_after_the_expected_prefix() {
    let stream = futures_util::stream::iter([
        Ok::<_, std::io::Error>("event: first\ndata: {}\n\n"),
        Ok("event: unexpected\ndata: {}\n\n"),
    ]);
    read_sse(Body::from_stream(stream), 1, false).await;
}

#[tokio::test]
#[should_panic(expected = "incomplete final SSE frame")]
async fn incomplete_trailing_sse_frames_are_rejected_at_eof() {
    read_sse(Body::from("data: {}\n\ndata: unfinished\n"), 1, false).await;
}

#[test]
fn raw_body_repeat_expands_exact_bytes_with_a_default_of_one() {
    let request = |extra: Value| {
        let mut value = json!({"method":"POST", "path":"/", "rawBody":"ab"});
        value
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        serde_json::from_value::<RequestFixture>(value).unwrap()
    };
    assert_eq!(
        encode_request_body(&request(json!({})), &Symbols::default()),
        b"ab"
    );
    assert_eq!(
        encode_request_body(&request(json!({"rawBodyRepeat":3})), &Symbols::default()),
        b"ababab"
    );
    for extra in [
        json!({"rawBodyRepeat":0}),
        json!({"rawBodyRepeat":MAX_RAW_BODY_REPEAT + 1}),
        json!({"rawBody":"12345", "rawBodyRepeat":MAX_RAW_BODY_REPEAT}),
        json!({"rawBody":null, "rawBodyRepeat":1}),
        json!({"body":{}}),
    ] {
        assert!(std::panic::catch_unwind(|| encode_request_body(
            &request(extra),
            &Symbols::default()
        ))
        .is_err());
    }
}
