use super::{ledger::hash, protocol::Signed};
use serde_json::{json, Value};
use std::time::Instant;

pub async fn post(
    client: &reqwest::Client,
    endpoint: &str,
    channel: &str,
    event_id: &str,
    signed: &Signed,
    ordinal: usize,
    started: Instant,
) -> Value {
    let request_id = uuid::Uuid::new_v4().to_string();
    let mut row = json!({"ordinal":ordinal,"request_sha256":hash(request_id.as_bytes()),"endpoint_sha256":hash(endpoint.as_bytes()),"start_ms":started.elapsed().as_millis()});
    let response = client
        .post(format!("{endpoint}/v1/callbacks/{channel}"))
        .header("x-request-id", &request_id)
        .header("content-type", "application/json")
        .header("x-nyxid-callback-token", &signed.callback)
        .header("x-nyxid-platform", "telegram")
        .header("x-nyxid-message-id", event_id)
        .header("x-nyxid-timestamp", &signed.timestamp)
        .body(signed.body.clone())
        .send()
        .await;
    match response {
        Ok(mut response) => {
            row["headers_ms"] = json!(started.elapsed().as_millis());
            let status = response.status().as_u16();
            row["status"] = status.into();
            let echoed = response
                .headers()
                .get("x-request-id")
                .and_then(|x| x.to_str().ok());
            row["request_id_echo_matches"] = json!(echoed == Some(request_id.as_str()));
            let mut body = Vec::new();
            let mut valid = true;
            loop {
                match response.chunk().await {
                    Ok(Some(chunk)) if body.len() + chunk.len() <= 16384 => {
                        body.extend_from_slice(&chunk)
                    }
                    Ok(None) => break,
                    _ => {
                        valid = false;
                        break;
                    }
                }
            }
            row["body_ms"] = json!(started.elapsed().as_millis());
            row["body_complete"] = valid.into();
            row["body_sha256"] = hash(&body).into();
            let value: Option<Value> = valid.then(|| serde_json::from_slice(&body).ok()).flatten();
            row["accepted_body"] = json!(value.as_ref().is_some_and(|v| v["status"] == "accepted"));
            if let Some(value) = value {
                if let Ok(problem) = serde_json::from_value::<cmaeg_core::problem::Problem>(value) {
                    row["public_code"] = problem.code.to_string().into();
                }
            }
        }
        Err(error) => {
            row["transport_outcome"] = if error.is_timeout() {
                "timeout"
            } else {
                "error"
            }
            .into();
            row["end_ms"] = json!(started.elapsed().as_millis());
        }
    }
    row
}
