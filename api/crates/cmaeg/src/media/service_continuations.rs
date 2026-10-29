//! Existing-Agent original files use the same source checks and bounded byte,
//! fetch and job budgets as ordinary media tools. No file is persisted in RAM
//! beyond the synchronous upload, or copied into a Gateway record.
use super::MediaService;
use crate::media::{
    body::Overflow,
    budget::InflightPermit,
    derive::DeriveError,
    source::{FetchRequest, SourceCredential},
    target::MediaTarget,
};
use bytes::Bytes;
use cmaeg_core::media_continuation::MAX_CONTINUATION_FILE_BYTES;
use tokio::time::Instant;

pub struct ContinuationOriginal {
    pub bytes: Bytes,
    _held: InflightPermit,
}
impl MediaService {
    pub async fn original_for_continuation(
        &self,
        target: &MediaTarget,
        index: usize,
        deadline: Instant,
    ) -> Result<ContinuationOriginal, DeriveError> {
        tokio::time::timeout_at(
            deadline,
            self.continuation_original_inner(target, index, deadline),
        )
        .await
        .map_err(|_| DeriveError::TimedOut)?
    }
    async fn continuation_original_inner(
        &self,
        target: &MediaTarget,
        index: usize,
        deadline: Instant,
    ) -> Result<ContinuationOriginal, DeriveError> {
        let refreshed = self
            .refresh_native(target, index)
            .await
            .map_err(DeriveError::from)?;
        let target = refreshed.unwrap_or_else(|| target.clone());
        if !self.limiter.fetch.check(&target.channel.id) {
            return Err(DeriveError::RateLimited);
        }
        let _slot = self
            .limiter
            .derive_slot(&target.channel.id, deadline)
            .await
            .ok_or(DeriveError::TimedOut)?;
        let _permit = self
            .permits
            .acquire(deadline)
            .await
            .ok_or(DeriveError::TimedOut)?;
        let cap = self
            .pipeline
            .limits
            .max_bytes
            .min(MAX_CONTINUATION_FILE_BYTES);
        let declared = target
            .attachment(index)
            .ok_or(DeriveError::NotFound)?
            .size_bytes
            .unwrap_or(cap)
            .min(cap);
        let kib = u32::try_from(declared.div_ceil(1024)).map_err(|_| DeriveError::TooLarge)?;
        if !self.limiter.download_kib.check_n(&target.channel.id, kib) {
            return Err(DeriveError::RateLimited);
        }
        let held = self
            .pipeline
            .inflight
            .acquire(cap, deadline)
            .await
            .ok_or(DeriveError::TimedOut)?;
        let attachment = target.attachment(index).ok_or(DeriveError::NotFound)?;
        let body = self
            .pipeline
            .sources
            .fetch(FetchRequest {
                channel: &target.channel,
                event: &target.event,
                activity: &target.activity,
                locator: attachment.source.as_ref(),
                declared_size: attachment.size_bytes,
                credential: SourceCredential::ReplyToken,
                cap,
                overflow: Overflow::Reject,
                deadline,
            })
            .await?;
        let actual_kib =
            u32::try_from(body.bytes.len().div_ceil(1024)).map_err(|_| DeriveError::TooLarge)?;
        if !self
            .limiter
            .download_kib
            .check_n(&target.channel.id, actual_kib.saturating_sub(kib))
        {
            return Err(DeriveError::RateLimited);
        }
        self.refresh_native(&target, index)
            .await
            .map_err(DeriveError::from)?;
        Ok(ContinuationOriginal {
            bytes: body.bytes,
            _held: held,
        })
    }
}
