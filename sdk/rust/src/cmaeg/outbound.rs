use super::{Cmaeg, types::*};
use crate::{Result, http::Request};
use reqwest::{Method, StatusCode};

/// Prepare/commit helper failure. A delivery id means commit may have happened;
/// inspect its durable receipt, never blindly repeat the message.
#[derive(Debug)]
pub struct OutboundSendError {
    /// Saved operation to inspect when commit may have happened.
    pub delivery_id: Option<String>,
    /// Underlying validation, protocol or transport failure.
    pub error: crate::Error,
}
impl std::fmt::Display for OutboundSendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.delivery_id.is_some() {
            "Read the prepared outbound delivery status; commit was not confirmed"
        } else {
            "Outbound preparation failed"
        })
    }
}
impl std::error::Error for OutboundSendError {}

impl Cmaeg {
    /// Save a reviewable operation without sending; reuse the key only for identical content.
    pub async fn prepare_outbound_message(
        &self,
        channel: &str,
        body: &PrepareOutboundMessage,
        key: &str,
    ) -> Result<OutboundReceipt> {
        if key.is_empty() || key.len() > 128 || !key.bytes().all(|b| (33..=126).contains(&b)) {
            return Err(crate::Error::validation(
                "idempotency key must contain 1..128 visible ASCII bytes",
            ));
        }
        let segments = ["v1", "channels", channel, "outbound-messages"];
        let mut request =
            Request::operation(Method::POST, &segments, Some(body), StatusCode::CREATED)?
                .with_command_key(Some(key))?;
        request.root = true;
        self.client.request(request).await
    }
    /// Read an owned operation without retrying its send.
    pub async fn get_outbound_message(
        &self,
        channel: &str,
        delivery: &str,
    ) -> Result<OutboundReceipt> {
        let segments = ["v1", "channels", channel, "outbound-messages", delivery];
        let mut request = Request::get(&segments);
        request.root = true;
        self.client.request(request).await
    }
    /// Cancel an owned operation only before its send was committed.
    pub async fn cancel_outbound_message(
        &self,
        channel: &str,
        delivery: &str,
    ) -> Result<OutboundReceipt> {
        let segments = ["v1", "channels", channel, "outbound-messages", delivery];
        let mut request =
            Request::operation(Method::DELETE, &segments, None::<&()>, StatusCode::OK)?;
        request.root = true;
        self.client.request(request).await
    }
    /// Explicitly claim one send attempt using the exact message from preparation.
    pub async fn commit_outbound_message(
        &self,
        channel: &str,
        delivery: &str,
        body: &OutboundMessage,
    ) -> Result<OutboundReceipt> {
        let segments = [
            "v1",
            "channels",
            channel,
            "outbound-messages",
            delivery,
            "send",
        ];
        let mut request = Request::operation(Method::POST, &segments, Some(body), StatusCode::OK)?;
        request.root = true;
        self.client.request(request).await
    }
    /// Exactly one prepare and at most one commit. No mutation is retried.
    pub async fn send_outbound_message(
        &self,
        channel: &str,
        body: &PrepareOutboundMessage,
        key: &str,
    ) -> std::result::Result<OutboundReceipt, OutboundSendError> {
        let prepared = self
            .prepare_outbound_message(channel, body, key)
            .await
            .map_err(|error| OutboundSendError {
                delivery_id: None,
                error,
            })?;
        if prepared.status != OutboundStatus::Prepared {
            return Ok(prepared);
        }
        self.commit_outbound_message(channel, &prepared.delivery_id, &body.message)
            .await
            .map_err(|error| OutboundSendError {
                delivery_id: Some(prepared.delivery_id),
                error,
            })
    }
}
