use super::*;
use crate::{
    config::UsageExportConfig,
    producer_token::{
        oauth::ClientCredentialsProvider, ProducerScope, ProducerTokenError, ProducerTokenProvider,
        StaticTokenProvider,
    },
};
use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

mod wire;

pub(super) struct NyxIdUsageTransport {
    http: reqwest::Client,
    base: reqwest::Url,
    tokens: Arc<dyn ProducerTokenProvider>,
}

impl NyxIdUsageTransport {
    pub(super) fn new(config: &UsageExportConfig) -> Result<Self, String> {
        let tokens: Arc<dyn ProducerTokenProvider> = match &config.client_credentials {
            Some((id, secret)) => {
                let read = |key| {
                    std::env::var(key)
                        .ok()
                        .filter(|v| !v.trim().is_empty())
                        .ok_or_else(|| {
                            "usage producer credential reference unavailable".to_string()
                        })
                };
                ClientCredentialsProvider::new(
                    &config.base_url,
                    read(id)?,
                    read(secret)?,
                    ProducerScope::Usage,
                )?
            }
            None => Arc::new(StaticTokenProvider(config.credential_ref.clone())),
        };
        Self::with_tokens(config.base_url.clone(), tokens)
    }

    pub(super) fn with_tokens(
        base: reqwest::Url,
        tokens: Arc<dyn ProducerTokenProvider>,
    ) -> Result<Self, String> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .connect_timeout(Duration::from_secs(2))
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .user_agent("CMA-Sandbox-Usage/1")
            .build()
            .map_err(|_| "cannot construct usage transport".to_string())?;
        Ok(Self { http, base, tokens })
    }

    async fn send(
        &self,
        method: reqwest::Method,
        path: &[&str],
        body: Option<&[u8]>,
        query: &[(&str, &str)],
    ) -> Result<Option<Vec<u8>>, TransportError> {
        let mut url = self.base.clone();
        url.path_segments_mut()
            .map_err(|_| TransportError::Invalid)?
            .pop_if_empty()
            .extend(path);
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(query.iter().copied());
        }
        let mut request = self
            .http
            .request(method.clone(), url)
            .header(reqwest::header::ACCEPT, "application/json");
        if let Some(bytes) = body {
            if bytes.is_empty() || bytes.len() > 512 * 1024 {
                return Err(TransportError::Invalid);
            }
            request = request
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(bytes.to_vec());
        }
        let retry = request.try_clone().ok_or(TransportError::Invalid)?;
        let token = self.tokens.token().await.map_err(token_error)?;
        let mut response = request
            .bearer_auth(token.bearer())
            .send()
            .await
            .map_err(|_| TransportError::Unavailable)?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            if let Some(fresh) = self.tokens.rejected(&token).await.map_err(token_error)? {
                drop(response);
                response = retry
                    .bearer_auth(fresh.bearer())
                    .send()
                    .await
                    .map_err(|_| TransportError::Unavailable)?;
            }
        }
        let status = response.status().as_u16();
        if status == 404 && method == reqwest::Method::GET {
            return Ok(None);
        }
        if status == 401 {
            return Err(TransportError::Unauthorized);
        }
        if status == 403 {
            return Err(TransportError::Forbidden);
        }
        if status == 429 {
            let seconds = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|value| {
                    value.parse().ok().or_else(|| {
                        httpdate::parse_http_date(value).ok().map(|at| {
                            at.duration_since(SystemTime::now())
                                .unwrap_or_default()
                                .as_secs()
                        })
                    })
                })
                .unwrap_or(30)
                .clamp(1, 3600);
            return Err(TransportError::RateLimited(seconds));
        }
        if status >= 500 {
            return Err(TransportError::Unavailable);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| TransportError::Unavailable)?
        {
            if bytes.len().saturating_add(chunk.len()) > 512 * 1024 {
                return Err(TransportError::Invalid);
            }
            bytes.extend_from_slice(&chunk);
        }
        if status == 409 {
            #[derive(serde::Deserialize)]
            struct ErrorCode {
                error: String,
            }
            let code = serde_json::from_slice::<ErrorCode>(&bytes).ok();
            if method == reqwest::Method::GET
                && path.first() == Some(&"usage-cursors")
                && code.as_ref().is_some_and(|value| value.error == "conflict")
            {
                return Err(TransportError::CursorChanged);
            }
            return Err(
                if code.is_some_and(|v| {
                    matches!(
                        v.error.as_str(),
                        "stream_not_open" | "predecessor_missing" | "statement_predecessor_missing"
                    )
                }) {
                    TransportError::Prerequisite
                } else {
                    TransportError::Conflict
                },
            );
        }
        if !matches!(status, 200 | 201) || (method == reqwest::Method::GET && status != 200) {
            return Err(TransportError::Invalid);
        }
        Ok(Some(bytes))
    }
}

fn completeness_response(bytes: &[u8]) -> Result<CompletenessReceipt, TransportError> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Response {
        receipt: CompletenessReceipt,
        validation: serde_json::Value,
    }
    let response: Response = serde_json::from_slice(bytes).map_err(|_| TransportError::Invalid)?;
    if !matches!(
        response.validation["status"].as_str(),
        Some("pending" | "validated" | "superseded" | "rejected")
    ) {
        return Err(TransportError::Invalid);
    }
    Ok(response.receipt)
}

fn token_error(error: ProducerTokenError) -> TransportError {
    match error {
        ProducerTokenError::Retryable(_) => TransportError::Unavailable,
        ProducerTokenError::Invalid(_) => TransportError::Unauthorized,
    }
}

#[async_trait::async_trait]
impl UsageTransport for NyxIdUsageTransport {
    async fn submit_completeness(
        &self,
        attempt: &CompletenessAttempt,
    ) -> Result<CompletenessReceipt, TransportError> {
        let bytes = self
            .send(
                reqwest::Method::POST,
                &["usage-completeness"],
                Some(&attempt.payload),
                &[],
            )
            .await?
            .ok_or(TransportError::Invalid)?;
        let receipt = completeness_response(&bytes)?;
        if !receipt.validates(attempt) {
            return Err(TransportError::Conflict);
        }
        Ok(receipt)
    }
    async fn completeness_receipt(
        &self,
        id: &str,
    ) -> Result<Option<CompletenessReceipt>, TransportError> {
        self.send(reqwest::Method::GET, &["usage-completeness", id], None, &[])
            .await?
            .map(|bytes| completeness_response(&bytes))
            .transpose()
    }
    async fn submit(&self, event: &FrozenUsageEvent) -> Result<Receipt, TransportError> {
        let bytes = self
            .send(
                reqwest::Method::POST,
                &["usage-events"],
                Some(&event.payload),
                &[],
            )
            .await?
            .ok_or(TransportError::Invalid)?;
        let receipt = wire::receipt(&bytes)?;
        receipt.validate(event)?;
        Ok(receipt)
    }
    async fn receipt(&self, event_id: &str) -> Result<Option<Receipt>, TransportError> {
        self.send(
            reqwest::Method::GET,
            &["usage-receipts", event_id],
            None,
            &[],
        )
        .await?
        .map(|bytes| wire::receipt(&bytes))
        .transpose()
    }
    async fn cursor(&self, stream_id: &str) -> Result<Option<Cursor>, TransportError> {
        let Some(bytes) = self
            .send(
                reqwest::Method::GET,
                &["usage-cursors", stream_id],
                None,
                &[],
            )
            .await?
        else {
            return Ok(None);
        };
        let (mut combined, mut next_page) = wire::cursor(&bytes, stream_id)?;
        let mut seen = std::collections::HashSet::new();
        while let Some(next) = next_page.take() {
            if seen.len() >= 8 || !seen.insert(next.clone()) {
                return Err(TransportError::Invalid);
            }
            let bytes = self
                .send(
                    reqwest::Method::GET,
                    &["usage-cursors", stream_id],
                    None,
                    &[("page", &next)],
                )
                .await?
                .ok_or(TransportError::CursorChanged)?;
            let (page, next) = wire::cursor(&bytes, stream_id)?;
            if page.contiguous_sequence != combined.contiguous_sequence
                || page.validated_sequence != combined.validated_sequence
                || page.highest_sequence != combined.highest_sequence
                || page.head_digest != combined.head_digest
                || page.validated_head_digest != combined.validated_head_digest
            {
                return Err(TransportError::CursorChanged);
            }
            combined.missing_ranges.extend(page.missing_ranges);
            next_page = next;
            combined.validate(stream_id)?;
        }
        Ok(Some(combined))
    }
}

#[cfg(test)]
mod tests;
