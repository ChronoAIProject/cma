//! Strict Responses adapter for automatic agent titles through one sandbox proxy.

use layer2_core::sandbox::PreparedAgentTitle;
use sha2::{Digest, Sha256};
use std::fmt;
use std::time::Duration;
mod prepared;

use super::model_request_budget::RequestBudgetContext;
use async_trait::async_trait;
use futures_util::StreamExt as _;
use layer2_core::sandbox::{
    AgentTitleGenerationError, AgentTitleGenerationRequest, AgentTitleGenerationResponse,
    AgentTitleGenerator, AGENT_TITLE_CALL_TIMEOUT_SECONDS,
};
use layer3_adapter::provider::ProviderEndpoint;
use reqwest::header::{
    HeaderMap, HeaderValue, ACCEPT, ACCEPT_ENCODING, AUTHORIZATION, CONTENT_ENCODING, CONTENT_TYPE,
};
use reqwest::{StatusCode, Url};
use serde::Serialize;

const RESPONSES_PATH: &str = "/v1/responses";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_REQUEST_BYTES: usize = 16 * 1024;
const MAX_RESPONSE_BYTES: usize = 16 * 1024;
const MAX_OUTPUT_TOKENS: u32 = 1024;

/// One HTTP client bound to the model-proxy endpoint of a single sandbox.
#[derive(Clone)]
pub(crate) struct HttpAgentTitleGenerator {
    endpoint: Url,
    deployment_model: String,
    client: reqwest::Client,
    maximum_response_body: usize,
    proxy_authorization: Option<HeaderValue>,
    request_budget: Option<RequestBudgetContext>,
}

impl fmt::Debug for HttpAgentTitleGenerator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HttpAgentTitleGenerator")
            .field("endpoint", &self.endpoint)
            .field("deployment_model", &self.deployment_model)
            .field("maximum_response_body", &self.maximum_response_body)
            .finish_non_exhaustive()
    }
}

impl HttpAgentTitleGenerator {
    /// Bind generation to the proxy URL resolved for the agent's durable sandbox.
    pub(crate) fn new(sandbox_proxy: &ProviderEndpoint, deployment_model: &str) -> Option<Self> {
        let sandbox_proxy_base_url = provider_endpoint_url(&sandbox_proxy.endpoint)?;
        Self::build(
            sandbox_proxy_base_url.as_str(),
            deployment_model,
            Duration::from_secs(AGENT_TITLE_CALL_TIMEOUT_SECONDS),
            MAX_RESPONSE_BYTES,
        )
    }

    fn build(
        sandbox_proxy_base_url: &str,
        deployment_model: &str,
        request_timeout: Duration,
        maximum_response_body: usize,
    ) -> Option<Self> {
        let endpoint = title_endpoint(sandbox_proxy_base_url)?;
        let deployment_model = nonempty(deployment_model)?.to_owned();
        if request_timeout.is_zero() || maximum_response_body == 0 {
            return None;
        }
        let client = reqwest::Client::builder()
            .retry(reqwest::retry::never())
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT.min(request_timeout))
            .timeout(request_timeout)
            .build()
            .ok()?;
        Some(Self {
            endpoint,
            deployment_model,
            client,
            maximum_response_body,
            proxy_authorization: None,
            request_budget: None,
        })
    }

    pub(super) fn with_proxy_authorization(mut self, value: &str) -> Option<Self> {
        let mut value = HeaderValue::from_str(nonempty(value)?).ok()?;
        value.set_sensitive(true);
        self.proxy_authorization = Some(value);
        Some(self)
    }

    pub(super) fn with_request_budget(mut self, context: RequestBudgetContext) -> Self {
        self.request_budget = Some(context);
        self
    }

    #[cfg(test)]
    fn fixture(
        sandbox_proxy_base_url: &str,
        deployment_model: &str,
        timeout: Duration,
        maximum_response_body: usize,
    ) -> Self {
        Self::build(
            sandbox_proxy_base_url,
            deployment_model,
            timeout,
            maximum_response_body,
        )
        .expect("the title-generator fixture configuration is valid")
    }

    async fn prepare_request(
        &self,
        request: AgentTitleGenerationRequest,
    ) -> Result<Box<dyn PreparedAgentTitle>, AgentTitleGenerationError> {
        let deadline =
            tokio::time::Instant::now() + Duration::from_secs(AGENT_TITLE_CALL_TIMEOUT_SECONDS);
        if nonempty(request.model()) != Some(self.deployment_model.as_str()) {
            return Err(AgentTitleGenerationError::unavailable());
        }
        let body = serde_json::to_vec(&ResponsesRequest {
            model: &self.deployment_model,
            instructions: request.system_prompt(),
            input: [
                ResponsesInput {
                    role: "user",
                    content: request.owner_message(),
                },
                ResponsesInput {
                    role: "assistant",
                    content: request.assistant_message(),
                },
            ],
            stream: false,
            store: false,
            max_output_tokens: MAX_OUTPUT_TOKENS,
            reasoning: TitleReasoning { effort: "low" },
        })
        .map_err(|_| AgentTitleGenerationError::unavailable())?;
        if body.len() > MAX_REQUEST_BYTES {
            return Err(AgentTitleGenerationError::unavailable());
        }

        let mut authorization = HeaderValue::from_str(&format!("Bearer {}", request.bearer()))
            .map_err(|_| AgentTitleGenerationError::unavailable())?;
        authorization.set_sensitive(true);
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, authorization);
        if let Some(value) = &self.proxy_authorization {
            headers.insert("open-sandbox-api-key", value.clone());
        }
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));

        let permit = match &self.request_budget {
            Some(budget) => budget
                .reserve(&body)
                .await
                .map_err(AgentTitleGenerationError::budget)?,
            None => None,
        };

        let request_digest = format!("{:x}", Sha256::digest(&body));
        let frozen = self
            .client
            .post(self.endpoint.clone())
            .headers(headers)
            .body(body)
            .build()
            .map_err(|_| AgentTitleGenerationError::unavailable())?;
        Ok(Box::new(prepared::Prepared {
            generator: self.clone(),
            request,
            frozen,
            permit,
            deadline,
            request_digest,
        }))
    }

    async fn receive(
        &self,
        request: &AgentTitleGenerationRequest,
        response: reqwest::Response,
        permit: Option<super::model_request_budget::RequestPermit>,
    ) -> Result<AgentTitleGenerationResponse, AgentTitleGenerationError> {
        if response.status() != StatusCode::OK {
            let status = response.status();
            let mut body = Vec::new();
            let mut stream = response.bytes_stream();
            while let Some(Ok(chunk)) = stream.next().await {
                if body.len() + chunk.len() > self.maximum_response_body {
                    break;
                }
                body.extend_from_slice(&chunk);
            }
            let failure = generation_failure(status, &body);
            // Status only: the body may echo request content (#2266).
            tracing::warn!(
                status = status.as_u16(),
                permanent = failure.is_permanent(),
                "agent title provider request refused"
            );
            return Err(failure);
        }
        if response.headers().get_all(CONTENT_TYPE).iter().count() != 1
            || response
                .headers()
                .get(CONTENT_TYPE)
                .is_none_or(|value| !is_utf8_json(value))
            || response.headers().contains_key(CONTENT_ENCODING)
            || headers_contain(response.headers(), request.bearer().as_bytes())
            || self
                .proxy_authorization
                .as_ref()
                .is_some_and(|value| headers_contain(response.headers(), value.as_bytes()))
        {
            return Err(AgentTitleGenerationError::unavailable());
        }

        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| AgentTitleGenerationError::unavailable())?;
            let size = body
                .len()
                .checked_add(chunk.len())
                .ok_or_else(AgentTitleGenerationError::unavailable)?;
            if size > self.maximum_response_body {
                return Err(AgentTitleGenerationError::unavailable());
            }
            body.extend_from_slice(&chunk);
        }
        if contains_bytes(&body, request.bearer().as_bytes())
            || self
                .proxy_authorization
                .as_ref()
                .is_some_and(|value| contains_bytes(&body, value.as_bytes()))
        {
            return Err(AgentTitleGenerationError::unavailable());
        }
        if let Some(permit) = permit {
            permit
                .settle(&body)
                .await
                .map_err(AgentTitleGenerationError::budget)?;
        }
        let title = response_title(&body).ok_or_else(AgentTitleGenerationError::unavailable)?;
        if contains_bytes(title.as_bytes(), request.bearer().as_bytes())
            || self
                .proxy_authorization
                .as_ref()
                .is_some_and(|value| contains_bytes(title.as_bytes(), value.as_bytes()))
        {
            return Err(AgentTitleGenerationError::unavailable());
        }
        Ok(AgentTitleGenerationResponse::new(title))
    }
}

#[async_trait]
impl AgentTitleGenerator for HttpAgentTitleGenerator {
    async fn prepare(
        &self,
        request: AgentTitleGenerationRequest,
    ) -> Result<Box<dyn PreparedAgentTitle>, AgentTitleGenerationError> {
        self.prepare_request(request).await
    }

    async fn generate(
        &self,
        request: AgentTitleGenerationRequest,
    ) -> Result<AgentTitleGenerationResponse, AgentTitleGenerationError> {
        self.prepare_request(request).await?.dispatch().await
    }
}

#[derive(Serialize)]
struct ResponsesRequest<'a> {
    model: &'a str,
    instructions: &'static str,
    input: [ResponsesInput<'a>; 2],
    stream: bool,
    store: bool,
    max_output_tokens: u32,
    reasoning: TitleReasoning,
}

#[derive(Serialize)]
struct TitleReasoning {
    effort: &'static str,
}

#[derive(Serialize)]
struct ResponsesInput<'a> {
    role: &'static str,
    content: &'a str,
}

fn provider_endpoint_url(value: &str) -> Option<Url> {
    let value = value.trim().trim_end_matches('/');
    if value.is_empty() || value.starts_with('/') {
        return None;
    }
    let candidate = if value.contains("://") {
        value.to_owned()
    } else {
        format!("https://{value}")
    };
    let endpoint = Url::parse(&candidate).ok()?;
    title_endpoint(endpoint.as_str()).map(|_| endpoint)
}

fn title_endpoint(base_url: &str) -> Option<Url> {
    let base = Url::parse(base_url.trim_end_matches('/')).ok()?;
    if !matches!(base.scheme(), "http" | "https")
        || base.host_str().is_none()
        || !base.username().is_empty()
        || base.password().is_some()
        || base.query().is_some()
        || base.fragment().is_some()
    {
        return None;
    }
    let path = base.path().trim_end_matches('/');
    let suffix = match path {
        // NyxID's scoped model service uses the same relative path as managed Codex.
        "/api/v1/proxy/s/chrono-llm-public" | "/api/v1/proxy/s/chrono-llm" => "/responses",
        _ => RESPONSES_PATH,
    };
    Url::parse(&format!("{}{suffix}", base.as_str().trim_end_matches('/'))).ok()
}

fn nonempty(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

fn is_utf8_json(value: &HeaderValue) -> bool {
    let Ok(value) = value.to_str() else {
        return false;
    };
    let Some((media_type, parameter)) = value.split_once(';') else {
        return value.trim_ascii().eq_ignore_ascii_case("application/json");
    };
    let Some((name, charset)) = parameter.trim_ascii().split_once('=') else {
        return false;
    };
    media_type
        .trim_ascii()
        .eq_ignore_ascii_case("application/json")
        && name.eq_ignore_ascii_case("charset")
        && (charset.eq_ignore_ascii_case("utf-8") || charset.eq_ignore_ascii_case("\"utf-8\""))
}

fn response_title(body: &[u8]) -> Option<String> {
    let response: serde_json::Value = serde_json::from_slice(body).ok()?;
    if let Some(title) = response.get("output_text").and_then(|value| value.as_str()) {
        return Some(title.to_owned());
    }
    response
        .get("output")?
        .as_array()?
        .iter()
        .filter(|item| item.get("type").and_then(|value| value.as_str()) == Some("message"))
        .filter_map(|item| item.get("content").and_then(|value| value.as_array()))
        .flatten()
        .find(|content| {
            content.get("type").and_then(|value| value.as_str()) == Some("output_text")
        })?
        .get("text")?
        .as_str()
        .map(str::to_owned)
}

fn headers_contain(headers: &HeaderMap, protected: &[u8]) -> bool {
    headers.iter().any(|(name, value)| {
        contains_bytes(name.as_str().as_bytes(), protected)
            || contains_bytes(value.as_bytes(), protected)
    })
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack.len() >= needle.len()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

#[cfg(test)]
#[path = "titles_tests.rs"]
mod tests;

fn generation_failure(status: StatusCode, body: &[u8]) -> AgentTitleGenerationError {
    let denied_quota = serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .is_some_and(|body| {
            ["code", "type"].iter().any(|key| {
                matches!(
                    body.get("error")
                        .and_then(|e| e.get(key))
                        .and_then(|v| v.as_str()),
                    Some(
                        "insufficient_quota" | "billing_hard_limit_reached" | "billing_not_active"
                    )
                )
            })
        });
    if denied_quota
        || (status.is_client_error()
            && status != StatusCode::REQUEST_TIMEOUT
            && status != StatusCode::TOO_MANY_REQUESTS)
    {
        AgentTitleGenerationError::permanent()
    } else {
        AgentTitleGenerationError::unavailable()
    }
}
