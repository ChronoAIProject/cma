//! The replaceable IO boundary of Bot API calls.
use crate::net::exchange::{read_bounded, HttpReply, TransportError};
use futures_util::future::BoxFuture;
use reqwest::{header::HeaderMap, Client, Url};
use std::time::Duration;

/// The largest Bot API answer the client reads.
const ANSWER_MAX_BYTES: usize = 32_768;

/// Replaceable IO boundary of the Telegram client, mirroring `WebhookTransport`:
/// URL building, status mapping and answer parsing stay in the client. An
/// implementation honours `timeout` for the whole exchange.
pub trait TelegramTransport: Send + Sync {
    fn post(
        &self,
        url: Url,
        headers: HeaderMap,
        body: Vec<u8>,
        timeout: Duration,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>>;
}

/// The production Telegram transport: one reqwest request per call, whose
/// whole answer is read under the bound whatever its status.
pub struct HttpTransport {
    pub(super) http: Client,
}

impl TelegramTransport for HttpTransport {
    fn post(
        &self,
        url: Url,
        headers: HeaderMap,
        body: Vec<u8>,
        timeout: Duration,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>> {
        Box::pin(async move {
            let response = self
                .http
                .post(url)
                .headers(headers)
                .body(body)
                .timeout(timeout)
                .send()
                .await
                .map_err(|error| transport_failure(error.without_url()))?;
            let status = response.status();
            let headers = response.headers().clone();
            let body = read_bounded(response, ANSWER_MAX_BYTES).await;
            Ok(HttpReply {
                status,
                headers,
                body,
            })
        })
    }
}

/// Classifies a transport failure. It only receives errors already stripped
/// of their token-bearing URL, so a classifier that later inspects or logs
/// the error cannot expose the bot token.
fn transport_failure(_error: reqwest::Error) -> TransportError {
    TransportError::Unavailable
}
