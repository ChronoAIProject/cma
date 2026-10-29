use super::*;

/// Original response outcome; attachment responses can still arrive afterwards.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct OriginalMediaResponse {
    /// Canonical original turn text and problem.
    #[serde(flatten)]
    pub outcome: TurnResult,
    /// Original Manager response identity.
    pub response_id: String,
    /// True while attachment responses remain observable.
    pub media_pending: bool,
    /// Retained Gateway metadata URL; never followed automatically.
    pub media_responses_url: String,
}

/// Independent attachment response, correlated to the original turn.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaResponse {
    /// Original CloudEvent identity.
    pub event_id: String,
    /// Original transport conversation identity.
    pub conversation_id: String,
    /// Separate Manager response identity.
    pub response_id: String,
    /// Manager response URL requiring its own existing authority.
    pub response_url: String,
    /// Original Manager response identity.
    pub parent_response_id: String,
    /// Independent delivery receipt identity.
    pub delivery_id: String,
    /// Original frozen attachment slot, between zero and fifteen.
    pub attachment_index: u64,
    /// Original file or derived artifact kind.
    pub kind: MediaContinuationKind,
    /// Completed, failed, or incomplete response outcome.
    pub status: String,
    /// Complete response text, possibly empty on failure.
    pub text: String,
}

/// Final opted-in stream outcome, preserving the original turn content.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaTurnResult {
    /// Original response text and error, not the supplementary response text.
    #[serde(flatten)]
    pub outcome: TurnResult,
    /// Completed or incomplete attachment observation.
    pub status: String,
    /// Whether all attachment work completed successfully.
    pub media_complete: bool,
}

/// Explicit media-stream event set; legacy `GatewayEvent` remains unchanged.
#[derive(Clone, PartialEq)]
pub enum GatewayMediaEvent {
    /// Ordinary delta/message or terminal when media is not enabled on this channel.
    Legacy(GatewayEvent),
    /// Original response completed; continue observing supplementary responses.
    ResponseCompleted(OriginalMediaResponse),
    /// Original response failed; continue observing the final media outcome.
    ResponseFailed(OriginalMediaResponse),
    /// Separate supplementary response associated with one frozen attachment slot.
    MediaResponse(MediaResponse),
    /// Stream completed after attachment work, preserving original turn content.
    Completed(MediaTurnResult),
    /// Attachment work or original turn failed; original problem is retained.
    Failed(MediaTurnResult),
    /// Observation reached its bounded deadline; inspect retained channel Activity.
    MediaPending,
}

/// Media-aware stream. Dropping it stops observation without cancelling admitted work.
pub struct GatewayMediaStream(GatewayStream);

impl Cmaeg {
    /// Explicitly negotiate supplementary attachment responses. No replay or retry occurs.
    /// Configure a request timeout exceeding 120 seconds to observe the server deadline.
    pub async fn stream_event_with_media(
        &self,
        channel_id: &str,
        event: &CloudEvent,
        options: &EventOptions<'_>,
    ) -> Result<GatewayMediaStream> {
        self.open_gateway_stream(channel_id, event, options, true)
            .await
            .map(GatewayMediaStream)
    }
}

impl GatewayMediaStream {
    /// Read one validated frame. Original response completion does not close this stream.
    pub async fn next(&mut self) -> Option<Result<GatewayMediaEvent>> {
        let next = self.0.stream.as_mut()?.next().await;
        let result = match next {
            Some(Ok(frame)) if frame.explicit_id.is_none() => self.decode(&frame.event, frame.data),
            Some(Ok(_)) => Err(Error::protocol(
                "gateway streams do not support event IDs or replay",
            )),
            Some(Err(error)) => Err(error),
            None => Err(Error::transport(
                "gateway stream closed before a terminal outcome",
            )),
        };
        if matches!(
            result,
            Err(_)
                | Ok(GatewayMediaEvent::Completed(_)
                    | GatewayMediaEvent::Failed(_)
                    | GatewayMediaEvent::MediaPending
                    | GatewayMediaEvent::Legacy(
                        GatewayEvent::Completed(_)
                            | GatewayEvent::Failed(_)
                            | GatewayEvent::Backpressure
                    ))
        ) {
            self.0.stream = None;
        }
        Some(result)
    }

    fn decode(&mut self, name: &str, data: serde_json::Value) -> Result<GatewayMediaEvent> {
        let invalid = || Error::protocol("invalid gateway media stream outcome");
        match name {
            "cmaeg.media.pending" => {
                if data["status"] != "pending" || data["retrieval"] != "channel_activity" {
                    return Err(invalid());
                }
                Ok(GatewayMediaEvent::MediaPending)
            }
            "cmaeg.response.completed" | "cmaeg.response.failed" => {
                let value: OriginalMediaResponse =
                    serde_json::from_value(data).map_err(|_| invalid())?;
                self.0
                    .identity(&value.outcome.event_id, &value.outcome.conversation_id)?;
                if !value.media_pending
                    || (name == "cmaeg.response.completed"
                        && (value.outcome.message.is_some() || value.outcome.error.is_some()))
                    || (name == "cmaeg.response.failed" && value.outcome.error.is_none())
                {
                    return Err(invalid());
                }
                Ok(if name == "cmaeg.response.completed" {
                    GatewayMediaEvent::ResponseCompleted(value)
                } else {
                    GatewayMediaEvent::ResponseFailed(value)
                })
            }
            "cmaeg.media.response" => {
                let value: MediaResponse = serde_json::from_value(data).map_err(|_| invalid())?;
                self.0.identity(&value.event_id, &value.conversation_id)?;
                if value.attachment_index >= 16
                    || !matches!(value.status.as_str(), "completed" | "failed" | "incomplete")
                {
                    return Err(invalid());
                }
                Ok(GatewayMediaEvent::MediaResponse(value))
            }
            "cmaeg.completed" | "cmaeg.failed" if data.get("media_complete").is_some() => {
                let value: MediaTurnResult = serde_json::from_value(data).map_err(|_| invalid())?;
                self.0
                    .identity(&value.outcome.event_id, &value.outcome.conversation_id)?;
                let completed = name == "cmaeg.completed";
                if value.media_complete != completed
                    || value.status != if completed { "completed" } else { "incomplete" }
                    || (completed
                        && (value.outcome.error.is_some() || value.outcome.message.is_some()))
                {
                    return Err(invalid());
                }
                Ok(if completed {
                    GatewayMediaEvent::Completed(value)
                } else {
                    GatewayMediaEvent::Failed(value)
                })
            }
            _ => self.0.decode(name, data).map(GatewayMediaEvent::Legacy),
        }
    }
}
