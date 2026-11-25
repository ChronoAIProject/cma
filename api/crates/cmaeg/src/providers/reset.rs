//! Synchronous provider-owned reset; the gateway retains no operation state.
use super::{ProviderClient, ProviderError};
use cmaeg_core::ids::{BindingId, ConversationId, ProviderSlug};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ConversationReset {
    #[schema(min_length = 1, max_length = 128)]
    pub binding_id: String,
    #[schema(minimum = 1)]
    pub generation: u64,
    #[schema(min_length = 1, max_length = 128)]
    pub conversation_id: String,
    #[schema(minimum = 1)]
    pub incarnation: u64,
    #[schema(min_length = 1, max_length = 128)]
    pub previous_agent_id: String,
    #[schema(min_length = 1, max_length = 128)]
    pub previous_conversation_id: String,
    #[schema(min_length = 1, max_length = 128)]
    pub successor_agent_id: String,
    #[schema(schema_with = reserved_state)]
    pub state: String,
}
fn reserved_state() -> utoipa::openapi::schema::Object {
    utoipa::openapi::schema::ObjectBuilder::new()
        .schema_type(utoipa::openapi::schema::Type::String)
        .enum_values(Some(["reserved"]))
        .build()
}
impl ProviderClient {
    pub async fn reset_conversation(
        &self,
        provider: &ProviderSlug,
        binding: &BindingId,
        conversation: &ConversationId,
        key: &str,
        body: serde_json::Value,
    ) -> Result<ConversationReset, ProviderError> {
        let handle = self.handle(provider)?;
        let mut url = handle
            .entry
            .url(
                "conversation",
                &[
                    ("binding_id", binding.as_str()),
                    ("conversation_id", conversation.as_str()),
                ],
            )
            .map_err(|_| ProviderError::InvalidRequest("invalid conversation reset path".into()))?;
        url.path_segments_mut()
            .map_err(|_| ProviderError::InvalidRequest("invalid conversation reset path".into()))?
            .push("resets");
        if url.path()
            != format!("/api/v1/cmaeg/bindings/{binding}/conversations/{conversation}/resets")
        {
            return Err(ProviderError::InvalidRequest(
                "invalid conversation reset path".into(),
            ));
        }
        let expected_agent = body["expected_agent_id"]
            .as_str()
            .filter(|value| !value.is_empty() && value.len() <= 128)
            .ok_or_else(|| ProviderError::InvalidRequest("invalid expected Agent".into()))?;
        let bearer = self.reset_bearer(provider, &url).await?;
        let _permit = super::client::acquire(&handle.inflight).await?;
        let response = handle
            .client
            .post(url)
            .bearer_auth(bearer.as_str())
            .header("idempotency-key", key)
            .json(&body)
            .timeout(self.request_timeout)
            .send()
            .await
            .map_err(|_| {
                ProviderError::Unavailable(
                    "conversation reset response unavailable; retry the same key".into(),
                )
            })?;
        let status = response.status();
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .map(std::time::Duration::from_secs);
        let bytes = tokio::time::timeout(self.request_timeout, bounded_body(response))
            .await
            .map_err(|_| {
                ProviderError::Unavailable("conversation reset response unavailable".into())
            })??;
        if status != reqwest::StatusCode::OK {
            return Err(reset_response_error(status, retry_after, &bytes));
        }
        let result: ConversationReset = serde_json::from_slice(&bytes)
            .map_err(|_| ProviderError::ResponseMalformed("invalid conversation reset".into()))?;
        if result.binding_id != binding.as_str()
            || result.conversation_id != conversation.as_str()
            || result.generation == 0
            || result.incarnation == 0
            || result.previous_agent_id != expected_agent
            || result.previous_conversation_id.is_empty()
            || result.previous_conversation_id.len() > 128
            || result.successor_agent_id.is_empty()
            || result.successor_agent_id.len() > 128
            || result.successor_agent_id == result.previous_agent_id
            || result.state != "reserved"
        {
            return Err(ProviderError::ResponseMalformed(
                "conversation reset identity mismatch".into(),
            ));
        }
        Ok(result)
    }
}

pub(super) async fn bounded_body(
    mut response: reqwest::Response,
) -> Result<Vec<u8>, ProviderError> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| ProviderError::Unavailable("conversation reset response unavailable".into()))?
    {
        if bytes.len() + chunk.len() > 16_384 {
            return Err(ProviderError::ResponseMalformed(
                "conversation reset exceeds limit".into(),
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
pub(super) fn reset_response_error(
    status: reqwest::StatusCode,
    retry_after: Option<std::time::Duration>,
    bytes: &[u8],
) -> ProviderError {
    if status.is_success() {
        return ProviderError::ResponseMalformed("unexpected conversation reset status".into());
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return ProviderError::Busy { retry_after };
    }
    if !status.is_client_error() {
        return ProviderError::Unavailable(
            "conversation reset unavailable; retry the same key".into(),
        );
    }
    let value: serde_json::Value = match serde_json::from_slice(bytes) {
        Ok(value) => value,
        Err(_) => {
            return ProviderError::ResponseMalformed("invalid conversation reset problem".into())
        }
    };
    let code = value
        .get("code")
        .or_else(|| value.get("error").and_then(|error| error.get("code")))
        .or_else(|| value.get("type"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("provider_error");
    if status == reqwest::StatusCode::CONFLICT
        && matches!(
            code,
            "stale_response" | "session_busy" | "conversation_in_progress"
        )
    {
        ProviderError::Busy { retry_after }
    } else if status == reqwest::StatusCode::NOT_FOUND && code.contains("conversation") {
        ProviderError::ConversationMissing
    } else {
        ProviderError::Rejected {
            status: status.as_u16(),
            code: code.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{management_auth::CmaManagementAuth, ProviderRegistry};
    use axum::{
        extract::State,
        http::{HeaderMap, StatusCode},
        response::{IntoResponse, Response},
        Json, Router,
    };
    use cmaeg_core::channel::Secret;
    use serde_json::{json, Value};
    use std::{
        sync::{Arc, Mutex},
        time::Duration,
    };

    #[derive(Default)]
    struct Wire {
        status: u16,
        response: String,
        delay: bool,
        lose_once: bool,
        body_fault: u8,
        requests: Vec<(String, Value)>,
    }
    struct Fixture {
        state: Arc<Mutex<Wire>>,
        client: ProviderClient,
        task: tokio::task::JoinHandle<()>,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.task.abort();
        }
    }
    async fn receive(
        State(state): State<Arc<Mutex<Wire>>>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> Response {
        assert_eq!(headers["authorization"], "Bearer manager-fixture");
        let (status, response, delay, body_fault) = {
            let mut wire = state.lock().unwrap();
            wire.requests
                .push((headers["idempotency-key"].to_str().unwrap().into(), body));
            if std::mem::take(&mut wire.lose_once) {
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            }
            (
                wire.status,
                wire.response.clone(),
                wire.delay,
                wire.body_fault,
            )
        };
        if delay {
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        if body_fault == 3 {
            let chunks = futures_util::stream::iter([
                Ok::<_, std::io::Error>("x".repeat(10_000)),
                Ok("y".repeat(10_000)),
            ]);
            return (
                StatusCode::from_u16(status).unwrap(),
                axum::body::Body::from_stream(chunks),
            )
                .into_response();
        }
        if body_fault != 0 {
            use futures_util::StreamExt;
            let chunks = futures_util::stream::once(async {
                tokio::time::sleep(Duration::from_millis(10)).await;
                Ok::<_, std::io::Error>(axum::body::Bytes::from_static(b"{\"binding_id\":"))
            })
            .chain(futures_util::stream::once(async move {
                tokio::time::sleep(Duration::from_millis(10)).await;
                if body_fault == 2 {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                }
                Err(std::io::Error::other("fixture connection lost"))
            }));
            return (
                StatusCode::from_u16(status).unwrap(),
                axum::body::Body::from_stream(chunks),
            )
                .into_response();
        }
        (
            StatusCode::from_u16(status).unwrap(),
            [("location", "/redirect-target")],
            response,
        )
            .into_response()
    }
    fn receipt() -> Value {
        json!({"binding_id":"binding_one","generation":1,"conversation_id":"conversation_one",
            "incarnation":2,"previous_agent_id":"agent_old","previous_conversation_id":"internal_predecessor",
            "successor_agent_id":"agent_new","state":"reserved"})
    }
    impl Fixture {
        async fn new() -> Self {
            let state = Arc::new(Mutex::new(Wire {
                status: 200,
                response: receipt().to_string(),
                ..Default::default()
            }));
            let app = Router::new().fallback(receive).with_state(state.clone());
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let origin = format!("http://{}", listener.local_addr().unwrap());
            let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let entries = json!([{"slug":"cma_codex","base_url":origin,"kind":"responses_http","max_inflight":4,
                "paths":{"agent_card":"/.well-known/agent-card.json","binding":"/api/v1/cmaeg/bindings/{binding_id}",
                "conversation":"/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}","responses":"/api/v2/responses"}}]);
            let registry = ProviderRegistry::parse(&entries.to_string()).unwrap();
            let mut client = ProviderClient::new(
                registry,
                Duration::from_secs(1),
                Duration::from_millis(100),
                4,
            )
            .unwrap();
            client.management_auth =
                Some(CmaManagementAuth::new(&origin, Secret::new("manager-fixture")).unwrap());
            Self {
                state,
                client,
                task,
            }
        }
        async fn reset(&self) -> Result<ConversationReset, ProviderError> {
            self.client
                .reset_conversation(
                    &ProviderSlug::new("cma_codex").unwrap(),
                    &BindingId::new("binding_one").unwrap(),
                    &ConversationId::new("conversation_one").unwrap(),
                    &"a".repeat(64),
                    json!({"expected_agent_id":"agent_old","owner":"alice","organization":false,
                    "idempotency_binding_id":"binding_one","request_digest":"b".repeat(64)}),
                )
                .await
        }
    }

    #[tokio::test]
    async fn reset_uses_manager_and_retains_exact_request_after_uncertain_response() {
        let fixture = Fixture::new().await;
        fixture.state.lock().unwrap().lose_once = true;
        assert!(matches!(
            fixture.reset().await,
            Err(ProviderError::Unavailable(_))
        ));
        assert_eq!(fixture.state.lock().unwrap().requests.len(), 1);
        let received = fixture.reset().await.unwrap();
        assert_eq!(serde_json::to_value(received).unwrap(), receipt());
        let requests = &fixture.state.lock().unwrap().requests;
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0], requests[1]);
        assert_eq!(requests[0].1.as_object().unwrap().len(), 5);
    }

    #[tokio::test]
    async fn reset_rejects_invalid_receipts_and_non_200_success() {
        let fixture = Fixture::new().await;
        for (field, value) in [
            ("binding_id", json!("other")),
            ("conversation_id", json!("other")),
            ("previous_agent_id", json!("other")),
            ("previous_conversation_id", json!("")),
            ("previous_conversation_id", json!("x".repeat(129))),
            ("successor_agent_id", json!("")),
            ("successor_agent_id", json!("agent_old")),
            ("successor_agent_id", json!("x".repeat(129))),
            ("successor_agent_id", Value::Null),
            ("generation", json!(0)),
            ("incarnation", json!(0)),
            ("state", json!("ready")),
        ] {
            let mut body = receipt();
            body[field] = value;
            fixture.state.lock().unwrap().response = body.to_string();
            assert!(
                matches!(
                    fixture.reset().await,
                    Err(ProviderError::ResponseMalformed(_))
                ),
                "{field}"
            );
        }
        for body in ["{".into(), "x".repeat(16_385), "{}".into()] {
            fixture.state.lock().unwrap().response = body;
            assert!(matches!(
                fixture.reset().await,
                Err(ProviderError::ResponseMalformed(_))
            ));
        }
        fixture.state.lock().unwrap().response = receipt().to_string();
        for status in [201, 202, 204] {
            fixture.state.lock().unwrap().status = status;
            assert!(matches!(
                fixture.reset().await,
                Err(ProviderError::ResponseMalformed(_))
            ));
        }
    }

    #[tokio::test]
    async fn reset_does_not_retry_timeout_or_follow_redirects() {
        let fixture = Fixture::new().await;
        fixture.state.lock().unwrap().delay = true;
        assert!(matches!(
            fixture.reset().await,
            Err(ProviderError::Unavailable(_))
        ));
        assert_eq!(fixture.state.lock().unwrap().requests.len(), 1);
        fixture.state.lock().unwrap().delay = false;
        fixture.state.lock().unwrap().body_fault = 1;
        assert!(matches!(
            fixture.reset().await,
            Err(ProviderError::Unavailable(_))
        ));
        assert_eq!(fixture.state.lock().unwrap().requests.len(), 2);
        fixture.state.lock().unwrap().body_fault = 0;
        fixture.state.lock().unwrap().status = 307;
        assert!(matches!(
            fixture.reset().await,
            Err(ProviderError::Unavailable(_))
        ));
        assert_eq!(fixture.state.lock().unwrap().requests.len(), 3);
    }

    #[tokio::test]
    async fn reset_preserves_refusal_codes_and_requires_authority_before_io() {
        let mut fixture = Fixture::new().await;
        fixture.state.lock().unwrap().status = 409;
        fixture.state.lock().unwrap().response =
            json!({"code":"partition_reset_disabled","detail":"private upstream text"}).to_string();
        assert_eq!(
            fixture.reset().await.unwrap_err(),
            ProviderError::Rejected {
                status: 409,
                code: "partition_reset_disabled".into()
            }
        );
        fixture.client.management_auth = None;
        assert!(matches!(
            fixture.reset().await,
            Err(ProviderError::InvalidRequest(_))
        ));
        assert_eq!(fixture.state.lock().unwrap().requests.len(), 1);
    }
    #[tokio::test]
    async fn reset_bounds_chunked_stalled_truncated_and_oversized_error_bodies() {
        let fixture = Fixture::new().await;
        fixture.state.lock().unwrap().status = 409;
        for body in [
            "{".into(),
            "x".repeat(16_385),
            format!("{{\"code\":\"{}\"}}", "private".repeat(3000)),
        ] {
            fixture.state.lock().unwrap().response = body;
            assert!(matches!(
                fixture.reset().await,
                Err(ProviderError::ResponseMalformed(_))
            ));
        }
        for status in [200, 409] {
            fixture.state.lock().unwrap().status = status;
            for fault in [1, 2, 3] {
                fixture.state.lock().unwrap().body_fault = fault;
                let error = fixture.reset().await.unwrap_err();
                if fault == 3 {
                    assert!(matches!(error, ProviderError::ResponseMalformed(_)));
                } else {
                    assert!(matches!(error, ProviderError::Unavailable(_)));
                }
                assert!(!error.to_string().contains("private"));
            }
        }
        assert_eq!(fixture.state.lock().unwrap().requests.len(), 9);
    }
    #[tokio::test]
    async fn reset_classifies_surrounding_upstream_errors_without_trusting_details() {
        let fixture = Fixture::new().await;
        for (status, code) in [
            (404, "binding_not_found"),
            (400, "invalid_binding"),
            (409, "binding_not_ready"),
            (409, "partition_reset_unsupported"),
            (409, "unknown_code"),
            (401, "unauthorized"),
            (403, "forbidden"),
        ] {
            fixture.state.lock().unwrap().status = status;
            fixture.state.lock().unwrap().response =
                json!({"code":code,"detail":"private"}).to_string();
            assert_eq!(
                fixture.reset().await.unwrap_err(),
                ProviderError::Rejected {
                    status,
                    code: code.into()
                }
            );
        }
        fixture.state.lock().unwrap().status = 429;
        assert!(matches!(
            fixture.reset().await,
            Err(ProviderError::Busy { .. })
        ));
        fixture.state.lock().unwrap().status = 503;
        assert!(matches!(
            fixture.reset().await,
            Err(ProviderError::Unavailable(_))
        ));
    }
}
