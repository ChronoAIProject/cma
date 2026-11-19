//! Control admission is independent of optional observer admission.
use super::*;

pub(super) fn approval_reply(request: &Request, body: &[u8]) -> bool {
    request.method() == Method::POST
        && request.uri().path() == "/api/v1/agui/run"
        && serde_json::from_slice::<serde_json::Value>(body)
            .ok()
            .and_then(|body| {
                body.get("resume")
                    .and_then(|value| value.as_array())
                    .cloned()
            })
            .is_some_and(|resume| !resume.is_empty())
}

pub(super) async fn durable(next: Next, request: Request) -> Response {
    layer2_core::sandbox::agent_observation::durable_read_scope(next.run(request)).await
}

pub(super) async fn remote_fallback(response: Response, next: Next, request: Request) -> Response {
    if response.status() != StatusCode::TOO_MANY_REQUESTS {
        return response;
    }
    let (parts, body) = response.into_parts();
    let bytes = match to_bytes(body, 4096).await {
        Ok(bytes) => bytes,
        Err(_) => return http::unavailable(),
    };
    // Only this typed refusal proves the read owner never reached control
    // admission. Other errors preserve the original response and receipt.
    if serde_json::from_slice::<serde_json::Value>(&bytes)
        .is_ok_and(|value| value["code"] == "relay_observer_capacity_exhausted")
    {
        return durable(next, request).await;
    }
    Response::from_parts(parts, Body::from(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt;

    #[tokio::test]
    async fn only_proven_remote_observer_refusal_reaches_local_control_admission() {
        for (status, code, fallback) in [
            (
                StatusCode::TOO_MANY_REQUESTS,
                "relay_observer_capacity_exhausted",
                true,
            ),
            (StatusCode::TOO_MANY_REQUESTS, "input_rate_limited", false),
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "agent_owner_unavailable",
                false,
            ),
            (StatusCode::OK, "accepted", false),
        ] {
            let app = axum::Router::new()
                .route("/", axum::routing::post(|| async { "local control" }))
                .layer(axum::middleware::from_fn(
                    move |request: Request, next: Next| async move {
                        let mut response =
                            (status, axum::Json(serde_json::json!({"code":code}))).into_response();
                        response
                            .headers_mut()
                            .insert("x-cma-command-id", "original-receipt".parse().unwrap());
                        remote_fallback(response, next, request).await
                    },
                ));
            let response = app
                .oneshot(Request::post("/").body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                if fallback { StatusCode::OK } else { status }
            );
            if !fallback {
                assert_eq!(response.headers()["x-cma-command-id"], "original-receipt");
            }
            let body = to_bytes(response.into_body(), 4096).await.unwrap();
            if fallback {
                assert_eq!(body, "local control");
            } else {
                let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(body["code"], code);
            }
        }
    }
}
