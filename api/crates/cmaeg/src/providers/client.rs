//! Allowlisted provider HTTP operations and streaming Responses turns.

use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant},
};

use cmaeg_core::{
    channel::Secret,
    ids::{BindingId, ConversationId, ProviderSlug},
};
use futures_util::StreamExt;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot, OwnedSemaphorePermit, Semaphore};

use super::{
    client_types::{
        Accepted, BindingRequest, BindingResponse, ProviderError, ProviderHandle, TurnRequest,
        TurnStream, MAX_METADATA_KEYS, MAX_METADATA_KEY_BYTES, MAX_METADATA_VALUE_BYTES,
        MAX_SEMAPHORE_WAIT,
    },
    parse_event, parse_sse,
    probe::{classify_conformance, AgentCardProbe},
    responses::{response_id_from_frame, ResponsesEvent},
    ProviderRegistry,
};

/// One client pool per allowlisted provider.
pub struct ProviderClient {
    providers: BTreeMap<ProviderSlug, ProviderHandle>,
    request_timeout: Duration,
}

impl ProviderClient {
    pub fn new(
        registry: ProviderRegistry,
        connect_timeout: Duration,
        request_timeout: Duration,
        global_max_inflight: usize,
    ) -> Result<Self, ProviderError> {
        if global_max_inflight == 0 {
            return Err(ProviderError::InvalidRequest(
                "global provider concurrency must be positive".into(),
            ));
        }
        let mut providers = BTreeMap::new();
        for entry in registry.iter() {
            let mut builder = reqwest::Client::builder()
                .connect_timeout(connect_timeout)
                .pool_idle_timeout(Duration::from_secs(90))
                .pool_max_idle_per_host(64)
                .tcp_nodelay(true)
                .redirect(reqwest::redirect::Policy::none())
                .user_agent(format!("cmaeg/{}", env!("CARGO_PKG_VERSION")));
            if entry.base_url.scheme() == "http"
                && entry
                    .base_url
                    .host_str()
                    .is_some_and(|host| host.to_ascii_lowercase().ends_with(".svc"))
            {
                builder = builder.http2_prior_knowledge();
            }
            let client = builder.build().map_err(|error| {
                ProviderError::Unavailable(format!("build provider client: {error}"))
            })?;
            let max_inflight = entry.max_inflight.min(global_max_inflight);
            providers.insert(
                entry.slug.clone(),
                ProviderHandle {
                    entry: entry.clone(),
                    client,
                    inflight: Arc::new(Semaphore::new(max_inflight)),
                },
            );
        }
        Ok(Self {
            providers,
            request_timeout,
        })
    }

    pub fn from_config(config: &crate::Config) -> Result<Self, ProviderError> {
        Self::new(
            config.providers.clone(),
            config.provider_connect_timeout,
            config.provider_request_timeout,
            config.provider_max_inflight,
        )
    }

    pub fn provider_count(&self) -> usize {
        self.providers.len()
    }

    pub async fn bind(
        &self,
        provider: &ProviderSlug,
        binding_id: &BindingId,
        request: &BindingRequest,
    ) -> Result<BindingResponse, ProviderError> {
        let handle = self.handle(provider)?;
        let _permit = acquire(&handle.inflight).await?;
        let url = handle
            .entry
            .url("binding", &[("binding_id", binding_id.as_str())])
            .map_err(|error| ProviderError::InvalidRequest(error.to_string()))?;
        let body = json!({
            "schema_version": request.schema_version,
            "profile": request.profile,
            "profile_digest": request.profile_digest,
            "agent_key": request.agent_key.as_str(),
            "tools": request.tools,
        });
        let response = handle
            .client
            .put(url)
            .bearer_auth(request.agent_key.as_str())
            .json(&body)
            .timeout(self.request_timeout)
            .send()
            .await
            .map_err(transport_error)?;
        let response = self.success_or_error(response).await?;
        response
            .json::<BindingResponse>()
            .await
            .map_err(|error| ProviderError::ResponseMalformed(error.to_string()))
    }

    pub async fn unbind(
        &self,
        provider: &ProviderSlug,
        binding_id: &BindingId,
        agent_key: &Secret,
    ) -> Result<(), ProviderError> {
        let handle = self.handle(provider)?;
        let _permit = acquire(&handle.inflight).await?;
        let url = handle
            .entry
            .url("binding", &[("binding_id", binding_id.as_str())])
            .map_err(|error| ProviderError::InvalidRequest(error.to_string()))?;
        let response = handle
            .client
            .delete(url)
            .bearer_auth(agent_key.as_str())
            .timeout(self.request_timeout)
            .send()
            .await
            .map_err(transport_error)?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(());
        }
        self.success_or_error(response).await.map(|_| ())
    }

    pub async fn ensure_conversation(
        &self,
        provider: &ProviderSlug,
        binding_id: &BindingId,
        conversation_id: &ConversationId,
        agent_key: &Secret,
    ) -> Result<(), ProviderError> {
        self.conversation_request(provider, binding_id, conversation_id, agent_key, false)
            .await
    }

    pub async fn delete_conversation(
        &self,
        provider: &ProviderSlug,
        binding_id: &BindingId,
        conversation_id: &ConversationId,
        agent_key: &Secret,
    ) -> Result<(), ProviderError> {
        self.conversation_request(provider, binding_id, conversation_id, agent_key, true)
            .await
    }

    async fn conversation_request(
        &self,
        provider: &ProviderSlug,
        binding_id: &BindingId,
        conversation_id: &ConversationId,
        agent_key: &Secret,
        delete: bool,
    ) -> Result<(), ProviderError> {
        let handle = self.handle(provider)?;
        let _permit = acquire(&handle.inflight).await?;
        let url = handle
            .entry
            .url(
                "conversation",
                &[
                    ("binding_id", binding_id.as_str()),
                    ("conversation_id", conversation_id.as_str()),
                ],
            )
            .map_err(|error| ProviderError::InvalidRequest(error.to_string()))?;
        let request = if delete {
            handle
                .client
                .delete(url)
                .bearer_auth(agent_key.as_str())
                .send()
        } else {
            handle
                .client
                .put(url)
                .bearer_auth(agent_key.as_str())
                .json(&json!({}))
                .send()
        };
        let response = tokio::time::timeout(self.request_timeout, request)
            .await
            .map_err(|_| ProviderError::Unavailable("provider request timed out".into()))?
            .map_err(transport_error)?;
        self.success_or_error(response).await.map(|_| ())
    }

    pub async fn start_turn(
        &self,
        provider: &ProviderSlug,
        request: TurnRequest,
    ) -> Result<TurnStream, ProviderError> {
        validate_metadata(&request.metadata)?;
        let handle = self.handle(provider)?;
        let permit = acquire(&handle.inflight).await?;
        let url = handle
            .entry
            .url("responses", &[])
            .map_err(|error| ProviderError::InvalidRequest(error.to_string()))?;
        let body = json!({
            "conversation": request.conversation_id,
            "stream": true,
            "input": request.input,
            "metadata": request.metadata,
        });
        let request_started = Instant::now();
        tracing::debug!(
            target: "cmaeg.provider.request_started",
            provider = %provider,
            conversation = %request.conversation_id,
            "provider request started"
        );
        let response = handle
            .client
            .post(url)
            .bearer_auth(request.agent_key.as_str())
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .header("idempotency-key", request.idempotency_key.as_str())
            .json(&body)
            .timeout(self.request_timeout)
            .send()
            .await
            .map_err(transport_error)?;
        let response = self.success_or_error(response).await?;
        let (accepted_tx, accepted) = oneshot::channel();
        let (events_tx, events) = mpsc::channel(64);
        tokio::spawn(forward_turn(response, accepted_tx, events_tx, permit));
        Ok(TurnStream {
            accepted,
            events,
            request_started,
        })
    }

    pub async fn probe(&self, provider: &ProviderSlug) -> Result<AgentCardProbe, ProviderError> {
        let handle = self.handle(provider)?;
        let _permit = acquire(&handle.inflight).await?;
        let url = handle
            .entry
            .url("agent_card", &[])
            .map_err(|error| ProviderError::InvalidRequest(error.to_string()))?;
        let started = Instant::now();
        let response = handle
            .client
            .get(url)
            .timeout(self.request_timeout)
            .send()
            .await
            .map_err(transport_error)?;
        let response = self.success_or_error(response).await?;
        let card = response
            .json::<Value>()
            .await
            .map_err(|error| ProviderError::ResponseMalformed(error.to_string()))?;
        Ok(AgentCardProbe {
            level: classify_conformance(&card),
            card,
            latency: started.elapsed(),
        })
    }

    fn handle(&self, provider: &ProviderSlug) -> Result<&ProviderHandle, ProviderError> {
        self.providers
            .get(provider)
            .ok_or_else(|| ProviderError::UnknownProvider(provider.to_string()))
    }

    async fn success_or_error(
        &self,
        response: reqwest::Response,
    ) -> Result<reqwest::Response, ProviderError> {
        if response.status().is_success() {
            return Ok(response);
        }
        Err(classify_error(response).await)
    }
}

async fn acquire(semaphore: &Arc<Semaphore>) -> Result<OwnedSemaphorePermit, ProviderError> {
    tokio::time::timeout(MAX_SEMAPHORE_WAIT, semaphore.clone().acquire_owned())
        .await
        .map_err(|_| ProviderError::Busy {
            retry_after: Some(MAX_SEMAPHORE_WAIT),
        })?
        .map_err(|_| ProviderError::Unavailable("provider semaphore closed".into()))
}

async fn forward_turn(
    response: reqwest::Response,
    accepted_tx: oneshot::Sender<Accepted>,
    events_tx: mpsc::Sender<ResponsesEvent>,
    _permit: OwnedSemaphorePermit,
) {
    let mut accepted_tx = Some(accepted_tx);
    let mut stream = Box::pin(parse_sse(response.bytes_stream()));
    while let Some(frame) = stream.next().await {
        let frame = match frame {
            Ok(frame) => frame,
            Err(error) => {
                tracing::warn!(%error, "provider SSE parser stopped");
                break;
            }
        };
        let event = match parse_event(&frame) {
            Ok(event) => event,
            Err(error) => {
                tracing::warn!(%error, "provider response event was rejected");
                break;
            }
        };
        if let Some(sender) = accepted_tx.take() {
            match &event {
                ResponsesEvent::Created { response_id } => {
                    let _ = sender.send(Accepted {
                        response_id: response_id.clone(),
                        at: Instant::now(),
                    });
                }
                ResponsesEvent::InProgress => {
                    let Some(response_id) =
                        response_id_from_frame(&frame).filter(|id| !id.is_empty())
                    else {
                        tracing::warn!("provider acceptance frame has no response id");
                        break;
                    };
                    let _ = sender.send(Accepted {
                        response_id,
                        at: Instant::now(),
                    });
                }
                _ => accepted_tx = Some(sender),
            }
        }
        let terminal = matches!(
            event,
            ResponsesEvent::Completed { .. }
                | ResponsesEvent::Failed { .. }
                | ResponsesEvent::Incomplete { .. }
        );
        if events_tx.send(event).await.is_err() {
            break;
        }
        if terminal {
            break;
        }
    }
}

fn validate_metadata(metadata: &BTreeMap<String, String>) -> Result<(), ProviderError> {
    if metadata.len() > MAX_METADATA_KEYS {
        return Err(ProviderError::InvalidRequest(format!(
            "metadata has more than {MAX_METADATA_KEYS} keys"
        )));
    }
    if metadata
        .keys()
        .any(|key| key.is_empty() || key.len() > MAX_METADATA_KEY_BYTES)
    {
        return Err(ProviderError::InvalidRequest(format!(
            "metadata keys must be 1..={MAX_METADATA_KEY_BYTES} bytes"
        )));
    }
    if metadata
        .values()
        .any(|value| value.len() > MAX_METADATA_VALUE_BYTES)
    {
        return Err(ProviderError::InvalidRequest(format!(
            "metadata values must be at most {MAX_METADATA_VALUE_BYTES} bytes"
        )));
    }
    Ok(())
}

fn transport_error(error: reqwest::Error) -> ProviderError {
    if error.is_timeout() {
        ProviderError::Unavailable("provider request timed out".into())
    } else {
        ProviderError::Unavailable("provider transport failed".into())
    }
}

async fn classify_error(response: reqwest::Response) -> ProviderError {
    let status = response.status();
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs);
    let code = response
        .bytes()
        .await
        .ok()
        .and_then(|bytes| {
            serde_json::from_slice::<Value>(&bytes[..bytes.len().min(64 * 1024)]).ok()
        })
        .and_then(|value| {
            value
                .get("code")
                .or_else(|| value.get("error").and_then(|error| error.get("code")))
                .or_else(|| value.get("type"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "provider_error".into());
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return ProviderError::Busy { retry_after };
    }
    if status == reqwest::StatusCode::CONFLICT
        && matches!(
            code.as_str(),
            "stale_response" | "session_busy" | "conversation_in_progress"
        )
    {
        return ProviderError::Busy { retry_after };
    }
    if status == reqwest::StatusCode::NOT_FOUND && code.contains("conversation") {
        return ProviderError::ConversationMissing;
    }
    if status.is_client_error() {
        ProviderError::Rejected {
            status: status.as_u16(),
            code,
        }
    } else {
        ProviderError::Unavailable(format!("provider returned HTTP {}", status.as_u16()))
    }
}
