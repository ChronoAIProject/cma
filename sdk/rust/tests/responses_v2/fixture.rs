use crate::wire::{Expected, headers};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub fn events() -> Vec<Value> {
    let corpus: Value = serde_json::from_str(include_str!(
        "../../../../api/crates/layer1-public/tests/fixtures/public-contract/responses-v2.json"
    ))
    .unwrap();
    let mut response = corpus["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == "responses-v2-retrieve")
        .unwrap()["response"]["body"]
        .clone();
    response["status"] = json!("queued");
    response["completed_at"] = Value::Null;
    let part = json!({"type":"output_text","text":"Hello 世界","annotations":[],"logprobs":[]});
    let item = json!({"type":"message","id":"msg_committed","status":"completed","role":"assistant","content":[part]});
    let mut completed = response.clone();
    completed["status"] = json!("completed");
    completed["completed_at"] = json!(2);
    completed["output"] = json!([item]);
    let mut progress = response.clone();
    progress["status"] = json!("in_progress");
    let mut empty_item = item.clone();
    empty_item["status"] = json!("in_progress");
    empty_item["content"] = json!([]);
    let mut empty_part = part.clone();
    empty_part["text"] = json!("");
    let mut events = vec![
        json!({"type":"response.created","response":response}),
        json!({"type":"response.in_progress","response":progress}),
        json!({"type":"response.output_item.added","output_index":0,"item":empty_item}),
        json!({"type":"response.content_part.added","output_index":0,"content_index":0,"item_id":"msg_committed","part":empty_part}),
        json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"item_id":"msg_committed","delta":"Hello 世界","logprobs":[]}),
        json!({"type":"response.output_text.done","output_index":0,"content_index":0,"item_id":"msg_committed","text":"Hello 世界","logprobs":[]}),
        json!({"type":"response.content_part.done","output_index":0,"content_index":0,"item_id":"msg_committed","part":part}),
        json!({"type":"response.output_item.done","output_index":0,"item":item}),
        json!({"type":"response.completed","response":completed}),
    ];
    for (seq, event) in events.iter_mut().enumerate() {
        event["sequence_number"] = seq.into();
    }
    events
}
pub fn wire(events: &[Value]) -> Vec<u8> {
    events
        .iter()
        .map(|event| {
            let id = if event["type"] == "error" {
                String::new()
            } else {
                format!("id: {}\n", event["sequence_number"])
            };
            format!(
                "{id}event: {}\ndata: {event}\n\n",
                event["type"].as_str().unwrap()
            )
        })
        .collect::<String>()
        .into_bytes()
}
pub fn expected(path: &str, body: Option<Value>, bytes: Vec<u8>) -> Expected {
    let mut request_headers = headers(true, "text/event-stream");
    if body.is_some() {
        request_headers.insert("content-type".into(), "application/json".into());
    }
    Expected {
        method: if body.is_some() { "POST" } else { "GET" }.into(),
        path: path.into(),
        headers: request_headers,
        body,
        status: 200,
        response_headers: BTreeMap::from([
            ("content-type".into(), "text/event-stream".into()),
            ("x-cma-response-id".into(), "resp_contract".into()),
            ("x-cma-command-id".into(), "cmd_contract".into()),
        ]),
        response: bytes,
        chunks: None,
        raw_body: None,
    }
}
