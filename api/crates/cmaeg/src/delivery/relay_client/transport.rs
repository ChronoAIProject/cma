//! HTTP transport for bounded NyxID relay exchanges.
use super::RECEIPT_MAX_BYTES;
use crate::{
    net::exchange::{HttpReply, TransportError},
    telemetry::turn_trace::{self, Operation, Outcome, Stage},
};
use futures_util::future::BoxFuture;
use reqwest::{header::HeaderMap, Client, StatusCode, Url};
use std::time::Duration;

/// Replaceable IO boundary of the relay client, mirroring `WebhookTransport`:
/// URL building, status mapping, `Retry-After` and receipt parsing stay in the
/// client. An implementation honours `timeout` for the whole exchange.
pub trait RelayTransport: Send + Sync {
    fn post(
        &self,
        url: Url,
        headers: HeaderMap,
        body: Vec<u8>,
        timeout: Duration,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>>;
}
/// The production relay transport: one reqwest request per call.
pub struct HttpTransport {
    http: Client,
}
impl HttpTransport {
    pub(super) fn new(timeout: Duration) -> Result<Self, crate::ConfigError> {
        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(timeout)
            .build()
            .map_err(|_| crate::ConfigError::InvalidManagement {
                detail: "cannot build relay client".into(),
            })?;
        Ok(Self { http })
    }
}
impl RelayTransport for HttpTransport {
    fn post(
        &self,
        url: Url,
        headers: HeaderMap,
        body: Vec<u8>,
        timeout: Duration,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>> {
        Box::pin(async move {
            let trace = turn_trace::start(Stage::ChannelPart);
            self.exchange(url, headers, body, timeout, trace).await
        })
    }
}
impl HttpTransport {
    async fn exchange(
        &self,
        url: Url,
        mut headers: HeaderMap,
        body: Vec<u8>,
        timeout: Duration,
        trace: Option<Operation>,
    ) -> Result<HttpReply, TransportError> {
        if let Some(trace) = &trace {
            trace.headers(&mut headers);
        }
        let response = self
            .http
            .post(url)
            .headers(headers)
            .body(body)
            .timeout(timeout)
            .send()
            .await
            .map_err(|_| TransportError::Unavailable)?;
        let status = response.status();
        let headers = response.headers().clone();
        // Only a 200 answer carries a receipt. Other statuses are mapped from
        // the status alone, so their response bodies are never read.
        let body = if status == StatusCode::OK {
            crate::net::exchange::read_bounded(response, RECEIPT_MAX_BYTES).await
        } else {
            Ok(Vec::new())
        };
        let reply = HttpReply {
            status,
            headers,
            body,
        };
        if let Some(trace) = trace {
            // HTTP completion does not attest platform delivery. A 502 can
            // follow a delivered multipart prefix and therefore stays unknown.
            trace.finish(
                if status.is_success() {
                    Outcome::Completed
                } else {
                    Outcome::Unknown
                },
                Some(status.as_u16()),
            );
        }
        Ok(reply)
    }
}

#[cfg(test)]
mod tests;
