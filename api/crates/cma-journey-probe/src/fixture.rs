//! Text-only model boundary for dedicated deterministic test Agents.

use std::net::SocketAddr;

use axum::{
    extract::DefaultBodyLimit,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde_json::{json, Value};
use tokio::net::TcpListener;

const PREFIX: &str = "CMA-CHECK-journey-";

pub async fn serve(address: SocketAddr) -> Result<(), &'static str> {
    if !address.ip().is_loopback() {
        return Err("scripted provider requires a loopback address");
    }
    let listener = TcpListener::bind(address)
        .await
        .map_err(|_| "cannot bind scripted provider")?;
    axum::serve(listener, router())
        .await
        .map_err(|_| "scripted provider failed")
}

fn router() -> Router {
    Router::new()
        .route("/v1/responses", post(respond))
        .layer(DefaultBodyLimit::max(1024 * 1024))
}

async fn respond(Json(request): Json<Value>) -> Response {
    let Some(marker) = marker(&request) else {
        tracing::warn!("scripted provider refused a request without a current slot marker");
        return StatusCode::UNPROCESSABLE_ENTITY.into_response();
    };
    let digest = crate::protocol::digest(marker);
    tracing::info!(slot_marker_digest = %digest, "scripted model invocation");
    let id = format!("resp_{}", &digest[..24]);
    let item = json!({"type":"message","id":format!("msg_{}", &digest[..24]),"role":"assistant","status":"completed",
        "content":[{"type":"output_text","text":marker,"annotations":[]}]});
    let frames = [
        json!({"type":"response.created","response":{"id":id,"status":"in_progress","output":[]}}),
        json!({"type":"response.output_item.added","output_index":0,"item":item}),
        json!({"type":"response.output_item.done","output_index":0,"item":item}),
        json!({"type":"response.completed","response":{"id":id,"status":"completed","output":[item],
            "usage":{"input_tokens":0,"output_tokens":0,"total_tokens":0}}}),
    ];
    let mut wire = String::new();
    for (sequence, mut frame) in frames.into_iter().enumerate() {
        use std::fmt::Write;
        frame["sequence_number"] = json!(sequence);
        let _ = write!(wire, "data: {frame}\n\n");
    }
    ([("content-type", "text/event-stream")], wire).into_response()
}

fn marker(request: &Value) -> Option<&str> {
    let text = match &request["input"] {
        Value::String(text) => text,
        Value::Array(items) => {
            let current = items.iter().rev().find(|item| item["role"] == "user")?;
            match &current["content"] {
                Value::String(text) => text,
                Value::Array(parts) => parts
                    .iter()
                    .filter(|part| part["type"] == "input_text")
                    .filter_map(|part| part["text"].as_str())
                    .find(|text| text.contains(PREFIX))?,
                _ => return None,
            }
        }
        _ => return None,
    };
    let offset = text.find(PREFIX)?;
    let length = PREFIX.len() + 64;
    let answer = text.get(offset..offset + length)?;
    let tail = text.get(offset + length..)?;
    let digest = &answer[PREFIX.len()..];
    if !digest
        .bytes()
        .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        || tail
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        || tail.contains(PREFIX)
    {
        return None;
    }
    Some(answer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn serves_only_current_slot_with_zero_synthetic_usage() {
        let marker_text = format!("{PREFIX}{}", "f".repeat(64));
        let request = |text: &str| json!({"input":[{"role":"user","content":[{"type":"input_text","text":text}] }]});
        let response = respond(Json(request(&marker_text))).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let body = std::str::from_utf8(&body).unwrap();
        assert!(body.contains(&marker_text) && body.contains("\"total_tokens\":0"));
        let mut stale = request(&marker_text);
        stale["input"]
            .as_array_mut()
            .unwrap()
            .push(json!({"role":"user","content":"ordinary input"}));
        assert_eq!(
            respond(Json(stale)).await.status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            serve("0.0.0.0:0".parse().unwrap()).await,
            Err("scripted provider requires a loopback address")
        );
    }
}
