use crate::discord::components_fixture::FixtureState;
use axum::http::StatusCode;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

type OnRead = Box<dyn FnOnce() + Send>;

pub(super) struct Native {
    pub message: Arc<Mutex<Value>>,
    pub fault: Arc<Mutex<Option<&'static str>>>,
    pub on_read: Arc<Mutex<Option<OnRead>>>,
}

pub(super) fn native(f: &FixtureState) -> Native {
    let state = Native {
        message: Arc::new(Mutex::new(Value::Null)),
        fault: Arc::new(Mutex::new(None)),
        on_read: Arc::new(Mutex::new(None)),
    };
    let current = state.message.clone();
    let fault = state.fault.clone();
    let on_read = state.on_read.clone();
    let next_id = Arc::new(Mutex::new(900_u64));
    let memory = Arc::downgrade(&f.rest.memory);
    let metadata = if f.thread_kind.is_some() {
        f.thread_metadata()
    } else {
        json!({"id":"99","type":1})
    };
    *f.rest.memory.on_call.lock().unwrap() = Some(Arc::new(move |call| {
        let mut status = StatusCode::OK;
        let body = if call.method == "PATCH"
            || (call.method == "POST" && call.path.ends_with("/messages"))
        {
            let multipart = call
                .content_type
                .as_ref()
                .is_some_and(|value| value.starts_with("multipart/"));
            let patch: Value = if multipart {
                payload(&call.body)
            } else {
                serde_json::from_slice(&call.body).unwrap()
            };
            let mut body = current.lock().unwrap();
            let old_attachments = body["attachments"].clone();
            if call.method == "POST" {
                *body =
                    json!({"id":"800","channel_id":"99","author":{"id":"77","bot":true},"type":19});
            }
            body["content"] = patch["content"].clone();
            body["components"] = patch["components"].clone();
            assert_eq!(
                patch["allowed_mentions"],
                json!({"parse":[],"replied_user":false})
            );
            if let Some(attachments) = patch.get("attachments") {
                let attachments = attachments.as_array().unwrap();
                let mut confirmed = Vec::new();
                for (index, attachment) in attachments.iter().enumerate() {
                    assert!(multipart);
                    assert_eq!(attachment["id"], index);
                    let wire = String::from_utf8_lossy(&call.body);
                    let marker = format!("name=\"files[{index}]\"");
                    assert_eq!(wire.matches(&marker).count(), 1);
                    let part = wire.split(&marker).nth(1).unwrap();
                    let bytes = part
                        .split("\r\n\r\n")
                        .nth(1)
                        .unwrap()
                        .split("\r\n--")
                        .next()
                        .unwrap();
                    let mime = part
                        .split("Content-Type: ")
                        .nth(1)
                        .or_else(|| part.split("content-type: ").nth(1))
                        .unwrap()
                        .split("\r\n")
                        .next()
                        .unwrap();
                    let mut next = attachment.clone();
                    let mut id = next_id.lock().unwrap();
                    next["id"] = id.to_string().into();
                    *id += 1;
                    next["size"] = bytes.len().into();
                    next["content_type"] = mime.into();
                    if attachment["is_spoiler"] == true
                        || attachment["filename"]
                            .as_str()
                            .is_some_and(|name| name.starts_with("SPOILER_"))
                    {
                        next["flags"] = 8.into();
                    }
                    confirmed.push(next);
                }
                body["attachments"] = Value::Array(confirmed);
            }
            let mut response = body.clone();
            if call.method == "PATCH" {
                match *fault.lock().unwrap() {
                    Some("500") => status = StatusCode::INTERNAL_SERVER_ERROR,
                    Some("old_membership") => response["attachments"] = old_attachments,
                    Some("reverse") => response["attachments"].as_array_mut().unwrap().reverse(),
                    Some("missing") => {
                        response["attachments"].as_array_mut().unwrap().pop();
                    }
                    Some("extra") => response["attachments"]
                        .as_array_mut()
                        .unwrap()
                        .push(json!({"id":"99999"})),
                    Some("duplicate") => {
                        response["attachments"][1]["id"] = response["attachments"][0]["id"].clone()
                    }
                    Some("last_size") => {
                        let last = response["attachments"]
                            .as_array_mut()
                            .unwrap()
                            .last_mut()
                            .unwrap();
                        last["size"] = 99.into();
                    }
                    Some("controls") => response["components"] = json!([]),
                    _ => {}
                }
            }
            response
        } else if call.method == "POST" {
            status = StatusCode::NO_CONTENT;
            Value::Null
        } else if call.path.ends_with("/messages/800") {
            if let Some(hook) = on_read.lock().unwrap().take() {
                hook();
            }
            current.lock().unwrap().clone()
        } else {
            metadata.clone()
        };
        memory
            .upgrade()
            .unwrap()
            .responses
            .lock()
            .unwrap()
            .push_back((status, body));
    }));
    state
}

fn payload(bytes: &[u8]) -> Value {
    let wire = String::from_utf8_lossy(bytes);
    serde_json::from_str(
        wire.split("name=\"payload_json\"\r\n\r\n")
            .nth(1)
            .unwrap()
            .split("\r\n--")
            .next()
            .unwrap(),
    )
    .unwrap()
}
