use super::{Cmaeg, sign_webhook, types::*};
use crate::{Error, Result, http::Request, stream::EventStream};
use reqwest::{Method, StatusCode, header};
use serde::{Deserialize, Serialize};

/// Runtime signing values; never print or persist this credential object.
pub struct WebhookAuth<'a> {
    /// Retained base64 secret string, used as UTF-8 without decoding.
    pub secret: &'a str,
    /// Stable webhook delivery identity.
    pub id: &'a str,
    /// Current Unix seconds as decimal text.
    pub timestamp: &'a str,
}

/// Custom event authorization. Omit webhook values to use the configured bearer.
#[derive(Default)]
pub struct EventOptions<'a> {
    /// Standard Webhooks signing values for HMAC sources.
    pub webhook: Option<WebhookAuth<'a>>,
}

fn request<'a>(
    segments: &'a [&'a str],
    event: &CloudEvent,
    options: &EventOptions<'_>,
    status: StatusCode,
) -> Result<Request<'a>> {
    let body = serde_json::to_vec(event)
        .map_err(|_| Error::validation("could not encode custom event"))?;
    if body.len() > 256 * 1024 {
        return Err(Error::validation("custom event exceeds 256 KiB"));
    }
    let mut request = Request::operation(Method::POST, segments, Some(event), status)?;
    request.root = true;
    request.headers.insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/cloudevents+json"),
    );
    if let Some(auth) = &options.webhook {
        let signed = sign_webhook(auth.secret, auth.id, auth.timestamp, &body)?;
        request.anonymous = true;
        for (name, value) in [
            ("webhook-id", signed.id),
            ("webhook-timestamp", signed.timestamp),
            ("webhook-signature", signed.signature),
        ] {
            let mut value = header::HeaderValue::from_str(&value)
                .map_err(|_| Error::validation("invalid webhook header"))?;
            value.set_sensitive(true);
            request.headers.insert(name, value);
        }
    }
    Ok(request)
}

impl Cmaeg {
    /// Admit a structured CloudEvent exactly once from this client call.
    pub async fn post_event(
        &self,
        channel_id: &str,
        event: &CloudEvent,
        options: &EventOptions<'_>,
    ) -> Result<EventAccepted> {
        let segments = ["v1", "channels", channel_id, "events"];
        self.client
            .request(request(&segments, event, options, StatusCode::ACCEPTED)?)
            .await
    }
    /// Observe one admitted custom event without reconnecting or retrying a write.
    pub async fn stream_event(
        &self,
        channel_id: &str,
        event: &CloudEvent,
        options: &EventOptions<'_>,
    ) -> Result<GatewayStream> {
        let segments = ["v1", "channels", channel_id, "events"];
        let query = [("mode", "stream".into())];
        let mut request = request(&segments, event, options, StatusCode::OK)?;
        request.query = &query;
        let stream = crate::stream::open_bounded(&self.client.inner, request, 1024 * 1024).await?;
        Ok(GatewayStream {
            stream: Some(stream),
            event_id: event.id.clone(),
            conversation_id: None,
        })
    }
}

/// Incremental custom-event text, with its stable event and conversation identity.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct TextDelta {
    /// Original CloudEvent ID.
    pub event_id: String,
    /// Opaque provider conversation ID.
    pub conversation_id: String,
    /// Incremental text, not the final response.
    pub text: String,
}

/// Full custom message or terminal outcome.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnResult {
    /// Original CloudEvent ID.
    pub event_id: String,
    /// Opaque provider conversation ID.
    pub conversation_id: String,
    /// Complete message/final text when present.
    pub text: Option<String>,
    /// Neutral message for a message event; absent on completion.
    pub message: Option<OutboundMessage>,
    /// Typed problem for a failed turn.
    pub error: Option<Problem>,
}

/// The four custom SSE event kinds and the documented terminal backpressure form.
#[derive(Clone, PartialEq)]
pub enum GatewayEvent {
    /// `cmaeg.text.delta`.
    TextDelta(TextDelta),
    /// `cmaeg.message`.
    Message(TurnResult),
    /// `cmaeg.completed`.
    Completed(TurnResult),
    /// `cmaeg.failed`, with a typed provider problem.
    Failed(TurnResult),
    /// `cmaeg.failed` with `internal` / `sync_backpressure`.
    Backpressure,
}

/// Bounded pull-based stream; dropping it cancels observation only.
pub struct GatewayStream {
    stream: Option<EventStream>,
    event_id: String,
    conversation_id: Option<String>,
}

impl GatewayStream {
    /// Read one validated event; EOF without a terminal outcome is a transport error.
    pub async fn next(&mut self) -> Option<Result<GatewayEvent>> {
        let next = self.stream.as_mut()?.next().await;
        let result = match next {
            Some(Ok(frame)) => {
                if frame.explicit_id.is_some() {
                    Err(Error::protocol(
                        "gateway streams do not support event IDs or replay",
                    ))
                } else {
                    self.decode(&frame.event, frame.data)
                }
            }
            Some(Err(error)) => Err(error),
            None => Err(Error::transport(
                "gateway stream closed before a terminal outcome",
            )),
        };
        if matches!(
            result,
            Err(_)
                | Ok(GatewayEvent::Completed(_)
                    | GatewayEvent::Failed(_)
                    | GatewayEvent::Backpressure)
        ) {
            self.stream = None;
        }
        Some(result)
    }

    fn decode(&mut self, name: &str, data: serde_json::Value) -> Result<GatewayEvent> {
        if name == "cmaeg.failed"
            && data["error"]["code"] == "internal"
            && data["error"]["detail"] == "sync_backpressure"
        {
            return Ok(GatewayEvent::Backpressure);
        }
        if name == "cmaeg.text.delta" {
            let value: TextDelta = serde_json::from_value(data)
                .map_err(|_| Error::protocol("invalid gateway text delta"))?;
            self.identity(&value.event_id, &value.conversation_id)?;
            return Ok(GatewayEvent::TextDelta(value));
        }
        let value: TurnResult = serde_json::from_value(data)
            .map_err(|_| Error::protocol("invalid gateway turn outcome"))?;
        self.identity(&value.event_id, &value.conversation_id)?;
        match name {
            "cmaeg.message" => Ok(GatewayEvent::Message(value)),
            "cmaeg.completed" if value.message.is_none() && value.error.is_none() => {
                Ok(GatewayEvent::Completed(value))
            }
            "cmaeg.failed" if value.error.is_some() => Ok(GatewayEvent::Failed(value)),
            _ => Err(Error::protocol(
                "unknown or inconsistent gateway stream event",
            )),
        }
    }

    fn identity(&mut self, event_id: &str, conversation_id: &str) -> Result<()> {
        if event_id != self.event_id
            || self
                .conversation_id
                .as_ref()
                .is_some_and(|id| id != conversation_id)
        {
            return Err(Error::protocol(
                "gateway stream changed its event or conversation identity",
            ));
        }
        self.conversation_id = Some(conversation_id.into());
        Ok(())
    }
}
