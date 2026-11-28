use super::*;

pub(super) async fn send(
    request: reqwest::RequestBuilder,
    replica: &Server,
    shared: bool,
    observer: usize,
) -> reqwest::Response {
    let request_id = uuid::Uuid::new_v4().to_string();
    let opened_at = layer2_core::now_rfc3339();
    let started = std::time::Instant::now();
    let result = request.header("x-request-id", &request_id).send().await;
    let elapsed_ms = started.elapsed().as_millis() as u64;
    let log = |event, http: Option<u16>, code: Option<&str>| {
        eprintln!(
            "response_observer_open {}",
            serde_json::json!({
                "event": event,
                "attempt_id": request_id,
                "request_id": request_id,
                "arm": if shared { "shared" } else { "unshared" },
                "observer_index": observer,
                "ingress_replica": replica.coordinator.instance_id,
                "opened_at": opened_at,
                "elapsed_ms": elapsed_ms,
                "http": http,
                "problem_code": code,
            })
        );
    };
    let response = result.unwrap_or_else(|error| {
        log("failed_before_headers", None, None);
        panic!("observer={observer} shared={shared} failed opening response: {error}");
    });
    let status = response.status();
    if status != reqwest::StatusCode::OK {
        let body = response.text().await.unwrap();
        let problem = serde_json::from_str::<Value>(&body).ok();
        let code = problem
            .as_ref()
            .and_then(|value| value["code"].as_str())
            .filter(|code| {
                code.len() <= 96
                    && !code.is_empty()
                    && code.bytes().all(|byte| {
                        byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'
                    })
            });
        log("headers", Some(status.as_u16()), code);
        panic!("observer={observer} shared={shared} returned HTTP {status}: {body}");
    }
    log("headers", Some(status.as_u16()), None);
    response
}
