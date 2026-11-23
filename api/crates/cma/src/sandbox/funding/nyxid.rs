//! NyxID #2204 producer transport; no NyxID wire details in lifecycle callers.
use super::*;
use crate::producer_token::{
    oauth::ClientCredentialsProvider, ProducerScope, ProducerTokenError, ProducerTokenProvider,
    StaticTokenProvider,
};
use futures_util::StreamExt;

pub(crate) struct NyxIdFundingProvider {
    config: SandboxBillingConfig,
    http: reqwest::Client,
    tokens: Arc<dyn ProducerTokenProvider>,
}
impl NyxIdFundingProvider {
    pub(crate) fn new(config: SandboxBillingConfig) -> Result<Arc<Self>, String> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("CMA-Sandbox-Funding/1")
            .build()
            .map_err(|_| "cannot construct funding HTTP client".to_string())?;
        let tokens: Arc<dyn ProducerTokenProvider> = match &config.client_credentials {
            Some((id, secret)) => {
                let read = |key| {
                    std::env::var(key)
                        .ok()
                        .filter(|value| !value.trim().is_empty())
                        .ok_or_else(|| {
                            "producer client credentials reference is unavailable".to_string()
                        })
                };
                ClientCredentialsProvider::new(
                    &config.base_url,
                    read(id)?,
                    read(secret)?,
                    ProducerScope::Funding,
                )?
            }
            None => Arc::new(StaticTokenProvider(config.credential_ref.clone())),
        };
        Ok(Arc::new(Self {
            config,
            http,
            tokens,
        }))
    }
    async fn send(
        &self,
        method: reqwest::Method,
        path: &[&str],
        body: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, FundingError> {
        let credential = self.tokens.token().await?;
        let mut url = self.config.base_url.clone();
        url.path_segments_mut()
            .map_err(|_| FundingError::Invalid("producer URL is invalid".into()))?
            .pop_if_empty()
            .extend(path);
        let mut request = self
            .http
            .request(method, url)
            .header(reqwest::header::ACCEPT, "application/json");
        // NyxID establishes the producer from its verified SA JWT. This local
        // identity reference is validated as configuration, never an authority header.
        if let Some(body) = body {
            let bytes = serde_json::to_vec(&body)
                .map_err(|_| FundingError::Invalid("cannot encode funding payload".into()))?;
            if bytes.len() > MAX_PROVIDER_RESPONSE_BYTES {
                return Err(FundingError::Invalid(
                    "funding payload exceeds 512 KiB".into(),
                ));
            }
            request = request
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(bytes);
        }
        // JSON is serialized once. Replays retain every durable idempotency identity.
        let retry = request
            .try_clone()
            .ok_or_else(|| FundingError::Invalid("funding request cannot be replayed".into()))?;
        let transport_error =
            |_| FundingError::Retryable("producer request timed out or transport failed".into());
        // reqwest appends headers: clone before authentication so a replay carries
        // exactly one Authorization value, belonging to the refreshed token.
        let mut response = request
            .bearer_auth(credential.bearer())
            .send()
            .await
            .map_err(transport_error)?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            if let Some(token) = self.tokens.rejected(&credential).await? {
                drop(response);
                response = retry
                    .bearer_auth(token.bearer())
                    .send()
                    .await
                    .map_err(transport_error)?;
            }
        }
        let status = response.status();
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| {
                v.parse::<u64>().ok().or_else(|| {
                    httpdate::parse_http_date(v).ok().map(|at| {
                        at.duration_since(SystemTime::now())
                            .unwrap_or_default()
                            .as_secs()
                    })
                })
            })
            .unwrap_or(30)
            .max(1);
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk
                .map_err(|_| FundingError::Retryable("producer response interrupted".into()))?;
            if bytes.len().saturating_add(chunk.len()) > MAX_PROVIDER_RESPONSE_BYTES {
                return Err(FundingError::Invalid(
                    "producer response exceeds 512 KiB".into(),
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        let value = serde_json::from_slice::<serde_json::Value>(&bytes);
        // Match only the documented machine error code. Arbitrary upstream text,
        // including plain-text body rejections, must never reach durable status/logs.
        if !status.is_success() {
            let detail = format!("producer HTTP {}", status.as_u16());
            let code = value.as_ref().ok().and_then(|v| v["error"].as_str());
            // The published producer uses this envelope for its two explicit
            // admission gates. Match the complete refusal only on the money-free
            // quote endpoint; other dependency failures and any window request
            // must retain their fail-closed/recovery behavior. Never retain text.
            if path == ["quotes"]
                && status.as_u16() == 402
                && code == Some("billing_provider_unavailable")
                && value.as_ref().is_ok_and(|v| {
                    v["error_code"] == 11302
                        && matches!(
                            v["message"].as_str(),
                            Some(
                                "Billing provider unavailable: CMA paid admission is disabled"
                                    | "Billing provider unavailable: Sandbox owner is outside the billing rollout"
                            )
                        )
                })
            {
                return Err(FundingError::NotEnrolled);
            }
            return Err(match status.as_u16() {
                404 => FundingError::NotFound,
                429 => FundingError::RateLimited(retry_after),
                402 if matches!(
                    code,
                    Some("billing_provider_unavailable" | "billing_not_configured")
                ) =>
                {
                    FundingError::Retryable("producer funding dependency unavailable".into())
                }
                402 | 409 => FundingError::Unfunded(detail),
                400 | 401 | 403 | 413 | 415 | 422 => FundingError::Invalid(detail),
                _ => FundingError::Retryable(detail),
            });
        }
        value.map_err(|_| FundingError::Invalid("producer response is not valid JSON".into()))
    }
}

fn allocation(value: &Allocation) -> serde_json::Value {
    serde_json::json!({"cpu_millis":value.cpu_millis,"memory_mib":value.memory_mib,"disk_gib":value.disk_gib})
}
fn field(value: &serde_json::Value, key: &str) -> Result<String, FundingError> {
    value[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| FundingError::Invalid(format!("producer response missing {key}")))
}
fn identity(value: &serde_json::Value, key: &str) -> Result<String, FundingError> {
    let text = field(value, key)?;
    if !uuid::Uuid::parse_str(&text)
        .is_ok_and(|id| id.get_version_num() == 4 && id.to_string() == text)
    {
        return Err(FundingError::Invalid(format!(
            "producer response has invalid {key}"
        )));
    }
    Ok(text)
}
fn integer(value: &serde_json::Value, key: &str) -> Result<u64, FundingError> {
    value[key]
        .as_u64()
        .filter(|n| *n <= i64::MAX as u64)
        .ok_or_else(|| FundingError::Invalid(format!("producer response has invalid {key}")))
}
fn parse_allocation(value: &serde_json::Value) -> Result<Allocation, FundingError> {
    let part = |key| {
        u32::try_from(integer(value, key)?)
            .map_err(|_| FundingError::Invalid("producer allocation overflow".into()))
    };
    let cpu_millis = part("cpu_millis")?;
    let memory_mib = part("memory_mib")?;
    Ok(Allocation {
        running: cpu_millis > 0 || memory_mib > 0,
        cpu_millis,
        memory_mib,
        disk_gib: part("disk_gib")?,
    })
}
fn window(value: serde_json::Value) -> Result<FundingWindow, FundingError> {
    Ok(FundingWindow {
        request_id: identity(&value, "request_id")?,
        quote_id: identity(&value, "quote_id")?,
        window_id: identity(&value, "window_id")?,
        owner_id: identity(&value, "owner_id")?,
        sandbox_id: field(&value, "sandbox_id")?,
        runtime_generation: integer(&value, "runtime_generation")?,
        allocation: parse_allocation(&value["allocation"])?,
        covered_from: integer(&value, "covered_from")?,
        covered_until: integer(&value, "covered_until")?,
        receipt: value,
    })
}

#[async_trait]
impl FundingProvider for NyxIdFundingProvider {
    async fn quote(&self, request: QuoteRequest) -> Result<FundingQuote, FundingError> {
        let value = self.send(reqwest::Method::POST,&["quotes"],Some(serde_json::json!({
            "owner_id":request.owner_id,"sandbox_id":request.sandbox_id,"allocation":allocation(&request.allocation)}))).await?;
        let accept_before = layer2_core::seconds_from_rfc3339(&field(&value, "accept_before")?)
            .ok_or_else(|| FundingError::Invalid("invalid quote expiry".into()))?;
        Ok(FundingQuote {
            quote_id: identity(&value, "quote_id")?,
            owner_id: identity(&value, "owner_id")?,
            sandbox_id: field(&value, "sandbox_id")?,
            maximum_allocation: parse_allocation(&value["maximum_allocation"])?,
            accept_before,
            receipt: value,
        })
    }
    async fn reserve(&self, request: ReserveRequest) -> Result<FundingWindow, FundingError> {
        window(self.send(reqwest::Method::POST,&["windows"],Some(serde_json::json!({
            "request_id":request.request_id,"quote_id":request.quote_id,"owner_id":request.owner_id,"sandbox_id":request.sandbox_id,
            "runtime_generation":request.runtime_generation,"operation_id":request.operation_id,"previous_window_id":request.previous_window_id,
            "seconds":request.seconds,"allocation":allocation(&request.allocation)}))).await?)
    }
    async fn recover(&self, request_id: &str) -> Result<Option<FundingWindow>, FundingError> {
        match self
            .send(reqwest::Method::GET, &["receipts", request_id], None)
            .await
        {
            Ok(value) => Ok(Some(window(value)?)),
            Err(FundingError::NotFound) => Ok(None),
            Err(error) => Err(error),
        }
    }
    async fn settle(&self, request: EvidenceRequest) -> Result<(), FundingError> {
        let intervals: Vec<_> = request.intervals.iter().map(|i| serde_json::json!({
            "revision":i.revision,"started_at":i.started_at,"ended_at":i.ended_at,"allocation":allocation(&i.allocation),
            "quantity":{"cpu_millis_seconds":i.quantity.cpu_millis_seconds,"memory_mib_seconds":i.quantity.memory_mib_seconds,"disk_gib_seconds":i.quantity.disk_gib_seconds}
        })).collect();
        let value = self.send(reqwest::Method::POST,&["windows",&request.window_id,"evidence"],Some(serde_json::json!({
            "owner_id":request.owner_id,"sandbox_id":request.sandbox_id,"runtime_generation":request.runtime_generation,"intervals":intervals}))).await?;
        if value["evidence_accepted"] != true
            || value["window_id"] != request.window_id
            || value["owner_id"] != request.owner_id
            || value["sandbox_id"] != request.sandbox_id
            || value["runtime_generation"] != request.runtime_generation
        {
            return Err(FundingError::Invalid(
                "evidence acknowledgement differs from the funded window".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod token_tests;

impl From<ProducerTokenError> for FundingError {
    fn from(error: ProducerTokenError) -> Self {
        match error {
            ProducerTokenError::Retryable(detail) => Self::Retryable(detail),
            ProducerTokenError::Invalid(detail) => Self::Invalid(detail),
        }
    }
}
