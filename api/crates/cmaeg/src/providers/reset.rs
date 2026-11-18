//! Synchronous provider-owned reset; the gateway retains no operation state.
use super::{ProviderClient, ProviderError};
use cmaeg_core::ids::{BindingId, ConversationId, ProviderSlug};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
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
        let mut response = self.success_or_error(response).await?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(ProviderError::ResponseMalformed(
                "unexpected conversation reset status".into(),
            ));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| {
            ProviderError::Unavailable("conversation reset response unavailable".into())
        })? {
            if bytes.len() + chunk.len() > 16384 {
                return Err(ProviderError::ResponseMalformed(
                    "conversation reset exceeds limit".into(),
                ));
            }
            bytes.extend_from_slice(&chunk);
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
        truncate: bool,
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
        let (status, response, delay, truncate) = {
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
                wire.truncate,
            )
        };
        if delay {
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        if truncate {
            use futures_util::StreamExt;
            let chunks = futures_util::stream::once(async {
                tokio::time::sleep(Duration::from_millis(10)).await;
                Ok::<_, std::io::Error>(axum::body::Bytes::from_static(b"{\"binding_id\":"))
            })
            .chain(futures_util::stream::once(async {
                tokio::time::sleep(Duration::from_millis(10)).await;
                Err(std::io::Error::other("fixture connection lost"))
            }));
            return axum::body::Body::from_stream(chunks).into_response();
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
        fixture.state.lock().unwrap().truncate = true;
        assert!(matches!(
            fixture.reset().await,
            Err(ProviderError::Unavailable(_))
        ));
        assert_eq!(fixture.state.lock().unwrap().requests.len(), 2);
        fixture.state.lock().unwrap().truncate = false;
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
}
