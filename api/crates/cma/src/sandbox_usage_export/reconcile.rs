use super::*;
use crate::store::sandbox_usage_export::CaptureStore;

pub(super) async fn inspect_cursor(
    store: &CaptureStore,
    event: &FrozenUsageEvent,
    cursor: &Cursor,
) -> Result<(), TransportError> {
    let sequence = cursor.validate(&event.stream_id)?;
    for (sequence, digest) in [
        (sequence, cursor.head_digest.as_ref()),
        (
            contract::integer(&cursor.validated_sequence)?,
            cursor.validated_head_digest.as_ref(),
        ),
    ] {
        let Some(digest) = digest else { continue };
        let local = store
            .event_digest(&event.stream_id, sequence)
            .await
            .map_err(|_| TransportError::Unavailable)?;
        if local.as_ref() != Some(digest) {
            return Err(TransportError::Conflict);
        }
    }
    Ok(())
}

pub(super) fn retry_seconds(generation: i64, retry_after: Option<u64>, jitter: u8) -> u64 {
    let exponent = u32::try_from(generation.saturating_sub(1))
        .unwrap_or(8)
        .min(8);
    ((5_u64 << exponent) + u64::from(jitter) % 5)
        .max(retry_after.unwrap_or(0))
        .clamp(1, 3600)
}
