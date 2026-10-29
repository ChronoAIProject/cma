//! Allowlisted provider HTTP operations and streaming Responses turns.

use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant},
};

use cmaeg_core::{
    channel::Secret,
    ids::{BindingId, ConversationId, ProviderSlug},
    profile::canonical_json,
};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot, OwnedSemaphorePermit, Semaphore};

use super::{
    client_types::{
        BindingRequest, BindingResponse, ProviderError, ProviderHandle, TurnRequest, TurnStream,
        MAX_METADATA_KEYS, MAX_METADATA_KEY_BYTES, MAX_METADATA_VALUE_BYTES, MAX_SEMAPHORE_WAIT,
    },
    probe::{classify_conformance, AgentCardProbe},
    stream::forward_turn,
    ProviderRegistry,
};

/// One client pool per allowlisted provider.
pub struct ProviderClient {
    providers: BTreeMap<ProviderSlug, ProviderHandle>,
    pub(super) request_timeout: Duration,
    pub(super) management_auth: Option<super::management_auth::CmaManagementAuth>,
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
                    responses: Arc::new(super::transport::ResponseTransport::new(
                        entry.base_url.clone(),
                        connect_timeout,
                    )),
                    inflight: Arc::new(Semaphore::new(max_inflight)),
                },
            );
        }
        Ok(Self {
            providers,
            request_timeout,
            management_auth: None,
        })
    }

    pub fn from_config(config: &crate::Config) -> Result<Self, ProviderError> {
        let mut client = Self::new(
            config.providers.clone(),
            config.provider_connect_timeout,
            config.provider_request_timeout,
            config.provider_max_inflight,
        )?;
        client.management_auth = Some(super::management_auth::CmaManagementAuth::new(
            &config.management.cma_base_url,
            config.management.cma_service_token.clone(),
        )?);
        Ok(client.with_service_auth(
            crate::service_auth::ServiceAuth::from_config(config)
                .map_err(|error| ProviderError::Unavailable(error.to_string()))?,
        ))
    }

    pub fn provider_count(&self) -> usize {
        self.providers.len()
    }

    /// Configured replay support only; this does not probe provider or platform health.
    pub fn response_recovery_providers(&self) -> Vec<String> {
        self.providers
            .iter()
            .filter(|(_, handle)| handle.entry.paths.recovery.is_some())
            .map(|(slug, _)| slug.as_str().to_owned())
            .collect()
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
        let mut body = json!({
            "schema_version": request.schema_version,
            "profile": request.profile,
            "profile_digest": request.profile_digest,
            "agent_key": request.agent_key.as_str(),
            "tools": request.tools,
        });
        if !request.target.is_profile() {
            body["target"] = serde_json::to_value(&request.target)
                .map_err(|error| ProviderError::InvalidRequest(error.to_string()))?;
        }
        let bearer = self
            .management_bearer(provider, &url, &request.agent_key)
            .await?;
        let response = handle
            .client
            .put(url)
            .bearer_auth(bearer.as_str())
            // Keep the reviewed provider bytes independent of serde_json's
            // workspace-unified preserve_order feature, including nested values.
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(canonical_json(body))
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
        let bearer = self.management_bearer(provider, &url, agent_key).await?;
        let response = handle
            .client
            .delete(url)
            .bearer_auth(bearer.as_str())
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
        self.conversation_request(
            provider,
            binding_id,
            conversation_id,
            agent_key,
            false,
            None,
        )
        .await
    }

    pub async fn ensure_conversation_at_generation(
        &self,
        provider: &ProviderSlug,
        binding_id: &BindingId,
        conversation_id: &ConversationId,
        agent_key: &Secret,
        generation: Option<u64>,
    ) -> Result<(), ProviderError> {
        if generation == Some(0) {
            return Err(ProviderError::InvalidRequest(
                "invalid binding generation".into(),
            ));
        }
        self.conversation_request(
            provider,
            binding_id,
            conversation_id,
            agent_key,
            false,
            generation,
        )
        .await
    }

    pub async fn delete_conversation(
        &self,
        provider: &ProviderSlug,
        binding_id: &BindingId,
        conversation_id: &ConversationId,
        agent_key: &Secret,
    ) -> Result<(), ProviderError> {
        self.conversation_request(provider, binding_id, conversation_id, agent_key, true, None)
            .await
    }

    async fn conversation_request(
        &self,
        provider: &ProviderSlug,
        binding_id: &BindingId,
        conversation_id: &ConversationId,
        agent_key: &Secret,
        delete: bool,
        generation: Option<u64>,
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
        let bearer = self.management_bearer(provider, &url, agent_key).await?;
        let mut request = if delete {
            handle.client.delete(url)
        } else {
            handle.client.put(url).json(&json!({}))
        };
        request = request.bearer_auth(bearer.as_str());
        if let Some(generation) = generation {
            request = request.header("x-cma-binding-generation", generation);
        }
        let request = request.send();
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
        self.start_with_permit(request, handle, permit).await
    }

    /// Runner admission never waits behind another turn's lifetime semaphore.
    pub async fn start_turn_immediate(
        &self,
        provider: &ProviderSlug,
        request: TurnRequest,
    ) -> Result<TurnStream, ProviderError> {
        validate_metadata(&request.metadata)?;
        let handle = self.handle(provider)?;
        let permit = handle
            .inflight
            .clone()
            .try_acquire_owned()
            .map_err(|_| ProviderError::Capacity)?;
        self.start_with_permit(request, handle, permit).await
    }

    async fn start_with_permit(
        &self,
        request: TurnRequest,
        handle: &ProviderHandle,
        permit: OwnedSemaphorePermit,
    ) -> Result<TurnStream, ProviderError> {
        let url = handle
            .entry
            .url("responses", &[])
            .map_err(|error| ProviderError::InvalidRequest(error.to_string()))?;
        if let Some(context) = &request.event_context {
            context.validate()?;
        }
        let mut body = json!({
            "conversation": request.conversation_id,
            "stream": true,
            "input": request.input,
            "metadata": request.metadata,
        });
        if let Some(context) = &request.event_context {
            body["event_context"] = serde_json::to_value(context)
                .map_err(|_| ProviderError::InvalidRequest("invalid event context".into()))?;
        }
        // Binding and turn requests share the original recursively sorted wire form.
        let bytes = canonical_json(body).into_bytes();
        let observed = tokio::time::timeout(
            self.request_timeout,
            handle.responses.send(
                url,
                request.agent_key.as_str(),
                request.idempotency_key.as_str(),
                bytes,
                request.timing,
            ),
        )
        .await
        .map_err(|_| ProviderError::Unavailable("provider request timed out".into()))??;
        let request_started = observed.written_at;
        let response = observed.response;
        let response = self.success_or_error(response).await?;
        let (accepted_tx, accepted) = oneshot::channel();
        let (events_tx, events) = mpsc::channel(64);
        tokio::spawn(forward_turn(
            response,
            accepted_tx,
            events_tx,
            permit,
            observed.complete,
        ));
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

    pub(super) fn handle(&self, provider: &ProviderSlug) -> Result<&ProviderHandle, ProviderError> {
        self.providers
            .get(provider)
            .ok_or_else(|| ProviderError::UnknownProvider(provider.to_string()))
    }

    pub(super) async fn success_or_error(
        &self,
        response: reqwest::Response,
    ) -> Result<reqwest::Response, ProviderError> {
        if response.status().is_success() {
            return Ok(response);
        }
        Err(classify_error(response).await)
    }
}

pub(super) async fn acquire(
    semaphore: &Arc<Semaphore>,
) -> Result<OwnedSemaphorePermit, ProviderError> {
    tokio::time::timeout(MAX_SEMAPHORE_WAIT, semaphore.clone().acquire_owned())
        .await
        .map_err(|_| ProviderError::Busy {
            retry_after: Some(MAX_SEMAPHORE_WAIT),
        })?
        .map_err(|_| ProviderError::Unavailable("provider semaphore closed".into()))
}

pub(super) fn validate_metadata(metadata: &BTreeMap<String, String>) -> Result<(), ProviderError> {
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

pub(super) fn transport_error(error: reqwest::Error) -> ProviderError {
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
