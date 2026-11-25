//! Allowlisted provider HTTP operations and streaming Responses turns.

use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant},
};

pub(super) use super::permits::acquire;
use super::permits::{Gate, Permit};
use cmaeg_core::{
    channel::Secret,
    ids::{BindingId, ConversationId, ProviderSlug},
    profile::canonical_json,
};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};

use super::{
    client_types::{
        ProviderError, ProviderHandle, TurnEvents, TurnRequest, TurnStream, MAX_METADATA_KEYS,
        MAX_METADATA_KEY_BYTES, MAX_METADATA_VALUE_BYTES,
    },
    probe::{classify_conformance, AgentCardProbe},
    stream::forward_turn,
    ProviderRegistry,
};

/// One client pool per allowlisted provider.
pub struct ProviderClient {
    pub(crate) execution_verifiers: Arc<crate::turns::execution_receipt::ExecutionVerifiers>,
    providers: BTreeMap<ProviderSlug, ProviderHandle>,
    pub(super) request_timeout: Duration,
    pub(super) management_auth: Option<super::management_auth::CmaManagementAuth>,
    worker_credentials: Option<crate::worker_credentials::WorkerCredentials>,
    tracking_required: bool,
    pub(super) turn_trace_enabled: bool,
    pub(super) context_diagnostics_enabled: bool,
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
                    inflight: Arc::new(Gate::new(entry.slug.to_string(), max_inflight)),
                },
            );
        }
        Ok(Self {
            execution_verifiers: Arc::default(),
            providers,
            request_timeout,
            management_auth: None,
            worker_credentials: None,
            tracking_required: false,
            turn_trace_enabled: false,
            context_diagnostics_enabled: false,
        })
    }

    pub fn from_config(config: &crate::Config) -> Result<Self, ProviderError> {
        let mut client = Self::new(
            config.providers.clone(),
            config.provider_connect_timeout,
            config.provider_request_timeout,
            config.provider_max_inflight,
        )?;
        client.context_diagnostics_enabled = super::context_retry::diagnostics::config::enabled()?;
        client.execution_verifiers =
            Arc::new(crate::turns::execution_receipt::ExecutionVerifiers::new(
                config.turns.provider_execution_attribution_enabled,
            ));
        client.turn_trace_enabled =
            crate::telemetry::turn_trace::config::enabled().map_err(|_| {
                ProviderError::InvalidRequest("invalid turn trace configuration".into())
            })?;
        client.management_auth = Some(super::management_auth::CmaManagementAuth::new(
            &config.management.cma_base_url,
            config.management.cma_service_token.clone(),
        )?);
        client.tracking_required = crate::worker_credentials::config::Settings::from_env()
            .map_err(|_| {
                ProviderError::InvalidRequest("invalid worker tracking configuration".into())
            })?
            .tracking;
        Ok(client.with_service_auth(
            crate::service_auth::ServiceAuth::from_config(config)
                .map_err(|error| ProviderError::Unavailable(error.to_string()))?,
        ))
    }

    pub fn provider_count(&self) -> usize {
        self.providers.len()
    }
    pub fn with_execution_verifiers(
        mut self,
        verifiers: Arc<crate::turns::execution_receipt::ExecutionVerifiers>,
    ) -> Self {
        self.execution_verifiers = verifiers;
        self
    }
    pub(crate) fn with_worker_credentials(
        mut self,
        tracking: Option<crate::worker_credentials::WorkerCredentials>,
    ) -> Self {
        self.worker_credentials = tracking;
        self
    }
    pub(super) async fn record_worker_key(&self, key: &Secret) -> Result<(), ProviderError> {
        match &self.worker_credentials {
            Some(tracking) => tracking.record_key(key.as_str()).await.map_err(|_| {
                ProviderError::Unavailable(
                    "durable worker registration failed; credential not forwarded".into(),
                )
            }),
            None if self.tracking_required => Err(ProviderError::Unavailable(
                "durable worker tracking unavailable; credential not forwarded".into(),
            )),
            None => Ok(()),
        }
    }

    /// Configured replay support only; this does not probe provider or platform health.
    pub fn response_recovery_providers(&self) -> Vec<String> {
        self.providers
            .iter()
            .filter(|(_, handle)| handle.entry.paths.recovery.is_some())
            .map(|(slug, _)| slug.as_str().to_owned())
            .collect()
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
            .try_acquire()
            .map_err(|_| ProviderError::Capacity)?;
        self.start_with_permit(request, handle, permit).await
    }

    async fn start_with_permit(
        &self,
        request: TurnRequest,
        handle: &ProviderHandle,
        permit: Permit,
    ) -> Result<TurnStream, ProviderError> {
        self.record_worker_key(&request.agent_key).await?;
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
        let negotiated = self.turn_trace_enabled
            && handle.entry.slug.as_str() == "nyxbot"
            && request.nyxid_trace;
        let mut trace = crate::telemetry::turn_trace::ProviderReader::new(
            self.turn_trace_enabled,
            negotiated,
            None,
        );
        if let Some(trace) = &mut trace {
            trace.begin_source();
        }
        let observed = tokio::time::timeout(
            self.request_timeout,
            handle.responses.send_with_trace(
                url,
                request.agent_key.as_str(),
                request.idempotency_key.as_str(),
                bytes,
                request.timing,
                negotiated,
            ),
        )
        .await
        .map_err(|_| ProviderError::Unavailable("provider request timed out".into()))??;
        let request_started = observed.written_at;
        let response = observed.response;
        if let Some(trace) = &mut trace {
            trace.response(response.headers(), response.status().as_u16());
        }
        let response = self.success_or_error(response).await?;
        let (accepted_tx, accepted) = oneshot::channel();
        let (events_tx, events) = mpsc::channel(64);
        let association = trace.as_ref().map(|_| Arc::new(std::sync::OnceLock::new()));
        tokio::spawn(forward_turn(
            response,
            accepted_tx,
            events_tx,
            permit,
            observed.complete,
            trace,
            association.clone(),
        ));
        Ok(TurnStream {
            trace: association,
            accepted,
            recovery: handle
                .entry
                .paths
                .recovery
                .as_ref()
                .map(|_| super::recovery::AcceptedRecovery::new(request.metadata)),
            events: TurnEvents(events),
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

    /// Refuse a provider that is not allowlisted, without contacting anyone.
    pub(crate) fn require_allowlisted(&self, provider: &ProviderSlug) -> Result<(), ProviderError> {
        self.handle(provider).map(|_| ())
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
