//! How a fetched original becomes a speech-to-text upload, and the transcript
//! pipeline that meters and charges it.
//!
//! [`AudioTranscoder`] decides what is uploaded and how many seconds it is
//! metered; [`NoTranscoder`] is the direct path that uploads the original as
//! it is. Charging happens in one place only, [`transcript_pipeline`], so a
//! transcoder that cuts or re-encodes audio changes no caller.
use super::{
    meter::{self, SpendRefusal},
    AudioContainer, AudioInput, Transcriber,
};
use crate::{
    media::{
        body::Overflow,
        derive::{expect, DeriveError, Derived, Pipeline},
        sniff::{Family, Sniffed},
        target::MediaTarget,
        text::{self, TextDocument},
    },
    telemetry::{self, names},
    SttConfig,
};
use bytes::Bytes;
use cmaeg_core::{
    activity::{Attachment, AttachmentKind, SourceKind},
    ids::ChannelId,
};
use futures_util::future::BoxFuture;
use std::sync::Arc;
use tokio::time::Instant;
use tracing::{field::Empty, Instrument};

/// What one transcription call uploads and what it costs.
#[derive(Debug)]
pub struct PlannedUpload {
    pub input: AudioInput,
    pub metered_seconds: u32,
    /// Set when the upload covers only the first N seconds of the original; `None` on the direct path.
    pub truncated_at_seconds: Option<u32>,
}

/// Turns a fetched original into the upload a transcriber receives.
pub trait AudioTranscoder: Send + Sync {
    /// Largest original this path accepts; it is also the fetch cap and the declared-size precheck.
    fn fetch_cap(&self) -> u64;
    /// `channel` lets an implementation queue work per channel (the transformer's FIFO); the direct path ignores it.
    fn plan<'a>(
        &'a self,
        original: Bytes,
        sniffed: Sniffed,
        channel: &'a ChannelId,
        deadline: Instant,
    ) -> BoxFuture<'a, Result<PlannedUpload, DeriveError>>;
}

/// The direct path: no transcoding. It is the production implementation until the transformer is configured,
/// and the transformer's fallback afterwards.
pub struct NoTranscoder {
    direct_cap: u64,
    transcriber: Arc<dyn Transcriber>,
}

impl NoTranscoder {
    /// The direct path under `config`'s cap, for the containers `transcriber`
    /// accepts.
    pub fn new(config: &SttConfig, transcriber: Arc<dyn Transcriber>) -> Self {
        Self {
            direct_cap: meter::direct_cap(config.max_bytes, config.max_seconds),
            transcriber,
        }
    }
}

impl AudioTranscoder for NoTranscoder {
    fn fetch_cap(&self) -> u64 {
        self.direct_cap
    }

    fn plan<'a>(
        &'a self,
        original: Bytes,
        sniffed: Sniffed,
        _channel: &'a ChannelId,
        _deadline: Instant,
    ) -> BoxFuture<'a, Result<PlannedUpload, DeriveError>> {
        let length = original.len() as u64;
        let planned = if length > self.direct_cap {
            Err(DeriveError::TooLong)
        } else {
            AudioContainer::from_sniffed(sniffed.mime)
                .filter(|container| self.transcriber.accepts(*container))
                .ok_or(DeriveError::UnsupportedFormat)
                .map(|container| PlannedUpload {
                    input: AudioInput {
                        bytes: original,
                        container,
                    },
                    // `length <= direct_cap <= CMAEG_STT_MAX_SECONDS × 4000`,
                    // so this never exceeds what one call may cost.
                    metered_seconds: meter::direct_seconds(length),
                    truncated_at_seconds: None,
                })
        };
        Box::pin(async move { planned })
    }
}

/// The transcript of attachment `index`: refusals that need no fetch, the
/// call rate, a capped fetch, the upload plan, the spend charge, the call
/// and the reconciled count, in that order.
///
/// The fetched bytes stay reserved against the pod's in-flight budget until
/// the transcriber returns. A charged call that later fails is not refunded,
/// which only errs towards spending less.
pub(crate) async fn transcript_pipeline(
    pipeline: &Pipeline,
    target: &MediaTarget,
    index: usize,
    deadline: Instant,
) -> Result<Derived, DeriveError> {
    if !pipeline.transcriber.enabled() {
        return Err(DeriveError::TranscriberDisabled);
    }
    let attachment = target.attachment(index).ok_or(DeriveError::NotFound)?;
    // CMAEG_MEDIA_MAX_BYTES bounds every attachment fetch, so an operator who
    // lowered it below the transcoder's cap is obeyed here too.
    let cap = pipeline
        .transcoder
        .fetch_cap()
        .min(pipeline.limits.max_bytes);
    if attachment.size_bytes.is_some_and(|declared| declared > cap) {
        return Err(DeriveError::TooLong);
    }
    let channel = &target.channel.id;
    let refused = |refusal: SpendRefusal| {
        count_refusal(target, attachment, refusal);
        DeriveError::RateLimited
    };
    pipeline.stt.admit_call(channel).map_err(refused)?;
    let (body, _held) = pipeline
        .fetch(target, index, cap, Overflow::Reject, deadline)
        .await
        .map_err(|error| match error {
            DeriveError::TooLarge => DeriveError::TooLong,
            other => other,
        })?;
    let sniffed = expect(&body, &[Family::Audio, Family::Video])?;
    let planned = pipeline
        .transcoder
        .plan(body.bytes, sniffed, channel, deadline)
        .await?;
    pipeline
        .stt
        .charge(channel, planned.metered_seconds)
        .map_err(refused)?;
    let metered = planned.metered_seconds;
    let transcript = call(pipeline, target, planned.input, metered, deadline).await?;
    let counted = pipeline
        .stt
        .reconcile(channel, metered, transcript.usage_seconds);
    metrics::counter!(names::STT_AUDIO_SECONDS).increment(u64::from(counted));
    Ok(Derived::Transcript {
        document: TextDocument::from_normalized(&text::normalize(&transcript.text), false),
        usage_seconds: transcript.usage_seconds,
        truncated_at_seconds: planned.truncated_at_seconds,
    })
}

/// One transcriber call inside the `cmaeg.media.stt` span, which names the
/// channel only by its hash and never carries a URL, key or transcript.
async fn call(
    pipeline: &Pipeline,
    target: &MediaTarget,
    input: AudioInput,
    metered_seconds: u32,
    deadline: Instant,
) -> Result<super::Transcript, DeriveError> {
    let span = tracing::info_span!(
        "cmaeg.media.stt",
        channel = %telemetry::channel_hash(target.channel.id.as_str()),
        metered_seconds,
        bytes = input.bytes.len(),
        container = input.container.format(),
        outcome = Empty,
        ms = Empty,
    );
    let started = Instant::now();
    let result = pipeline
        .transcriber
        .transcribe(input, deadline)
        .instrument(span.clone())
        .await
        .map_err(DeriveError::from);
    let outcome = result.as_ref().err().map_or("ok", DeriveError::as_str);
    span.record("outcome", outcome);
    span.record("ms", started.elapsed().as_millis() as u64);
    span.in_scope(|| tracing::info!("speech-to-text call finished"));
    result
}

/// Count a spend refusal against the item, by its activity source and kind.
fn count_refusal(target: &MediaTarget, attachment: &Attachment, refusal: SpendRefusal) {
    let source = match target.activity.source {
        SourceKind::Telegram { .. } => "telegram",
        SourceKind::Discord { .. } => "discord",
        SourceKind::NyxidRelay { .. } => "nyxid_relay",
        SourceKind::Custom => "custom",
    };
    let kind = match attachment.kind {
        AttachmentKind::Image => "image",
        AttachmentKind::File => "file",
        AttachmentKind::Audio => "audio",
        AttachmentKind::Video => "video",
        AttachmentKind::Other => "other",
    };
    metrics::counter!(
        names::MEDIA_ITEMS,
        names::SOURCE => source,
        names::KIND => kind,
        names::OUTCOME => refusal.outcome()
    )
    .increment(1);
    tracing::debug!(
        channel = %telemetry::channel_hash(target.channel.id.as_str()),
        outcome = refusal.outcome(),
        "speech-to-text refused by its spend limits"
    );
}
