use super::*;

pub(super) fn refuse(
    state: &IngressState,
    record: &RecordId,
    channel_id: &str,
    media_event_id: Option<&EventId>,
    reason: Refusal,
) -> Response {
    if let (Refusal::AttachmentUnsupported, Some(event_id)) = (reason, media_event_id) {
        state
            .activity
            .refused(record, SOURCE, Some(event_id), "attachment_unsupported");
        tracing::info!(
            source = SOURCE,
            channel = %crate::telemetry::channel_hash(channel_id),
            media_enabled = state.media_enabled,
            "Telegram relay attachment refused"
        );
        return refused(reason);
    }
    authenticated_refusal(state, record, SOURCE, reason)
}
