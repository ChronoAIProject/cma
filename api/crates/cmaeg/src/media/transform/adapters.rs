//! The transformer behind the media seams: [`TransformerKeyframes`] as the
//! visual extractor and [`TransformerTranscoder`] as the audio transcoder.
//!
//! Each keeps the implementation it replaces as its fallback: a failed
//! keyframe set falls back to a native Telegram thumbnail, and a failed
//! transcode to the direct upload when the original is eligible for it. Both
//! run inside the media service's single-flight derivation job, so the
//! tool's pending answer and the turn's preparation budget keep working.
use super::{
    outcome::{StderrClass, TransformOutcome},
    pcm::{pcm_seconds, PCM_BYTES_PER_SECOND, WAV_HEADER_BYTES},
    AudioRequest, KeyframeRequest, MediaTransformer, TransformError,
};
use crate::{
    media::{
        body::Overflow,
        derive::DeriveError,
        image::{self, NormalizedImage, PREVIEW},
        sniff::Sniffed,
        source::{FetchRequest, SourceCredential, SourceRegistry},
        transcribe::{
            upload::{AudioTranscoder, NoTranscoder, PlannedUpload},
            AudioContainer, AudioInput, Transcriber,
        },
        visual::{PlatformThumbnail, VisualExtractor, VisualKind, VisualRequest},
    },
    telemetry, SttConfig,
};
use bytes::Bytes;
use cmaeg_core::ids::ChannelId;
use futures_util::future::BoxFuture;
use std::sync::Arc;
use tokio::{
    sync::Semaphore,
    time::{timeout_at, Instant},
};

/// The agent-facing reason for a failed transform: one exhaustive mapping.
pub fn reason(error: TransformError) -> DeriveError {
    match error {
        TransformError::NoFrames => DeriveError::TimedOut,
        TransformError::Unavailable => DeriveError::TransformFailed,
        TransformError::Job {
            outcome,
            stderr_class,
        } => match outcome {
            TransformOutcome::Timeout | TransformOutcome::CpuLimit => DeriveError::TimedOut,
            TransformOutcome::ExitNonzero | TransformOutcome::OutputInvalid => {
                if stderr_class == StderrClass::UnsupportedCodec {
                    DeriveError::UnsupportedFormat
                } else {
                    DeriveError::DecodeFailed
                }
            }
            // An error never carries `ok`; were it to, the transform still
            // produced nothing usable.
            TransformOutcome::ResourceLimit
            | TransformOutcome::Killed
            | TransformOutcome::SandboxUnavailable
            | TransformOutcome::Ok => DeriveError::TransformFailed,
        },
    }
}

/// Keyframes of a video, normalized to previews, with the platform
/// thumbnail as the fallback.
pub struct TransformerKeyframes {
    transformer: Arc<dyn MediaTransformer>,
    sources: Arc<SourceRegistry>,
    decode: Arc<Semaphore>,
    thumbnail: PlatformThumbnail,
    max_bytes: u64,
    frames: u8,
}

impl TransformerKeyframes {
    /// `max_bytes` is `CMAEG_MEDIA_MAX_BYTES`, `frames` is
    /// `CMAEG_MEDIA_VIDEO_FRAMES`, and `decode` is the pod's one decode
    /// permit.
    pub fn new(
        transformer: Arc<dyn MediaTransformer>,
        sources: Arc<SourceRegistry>,
        decode: Arc<Semaphore>,
        max_bytes: u64,
        frames: u8,
    ) -> Self {
        Self {
            thumbnail: PlatformThumbnail::new(sources.clone(), decode.clone()),
            transformer,
            sources,
            decode,
            max_bytes,
            frames,
        }
    }

    /// The keyframe path alone: fetch the original, run the set and, once
    /// the slot is released, normalize each frame.
    ///
    /// The pod-wide reservation for the original is held by the caller (the
    /// derivation pipeline reserves `CMAEG_MEDIA_MAX_BYTES` for a keyframe
    /// extractor for as long as it runs), so none is taken here.
    async fn keyframes(
        &self,
        request: VisualRequest<'_>,
    ) -> Result<Vec<NormalizedImage>, DeriveError> {
        let target = request.target;
        let attachment = target
            .attachment(request.index)
            .ok_or(DeriveError::NotFound)?;
        let body = self
            .sources
            .fetch(FetchRequest {
                channel: &target.channel,
                event: &target.event,
                activity: &target.activity,
                locator: attachment.source.as_ref(),
                declared_size: attachment.size_bytes,
                credential: SourceCredential::ReplyToken,
                cap: self.max_bytes,
                overflow: Overflow::Reject,
                deadline: request.deadline,
            })
            .await?;
        let set = timeout_at(
            request.deadline,
            self.transformer.keyframes(KeyframeRequest {
                channel: target.channel.id.clone(),
                input: body.bytes,
                frames: self.frames,
            }),
        )
        .await
        .map_err(|_| DeriveError::TimedOut)?
        .map_err(reason)?;
        let mut images = Vec::with_capacity(set.frames.len());
        let mut failure = None;
        for (jpeg, at_ms) in set.frames {
            match image::normalize(jpeg, PREVIEW, &self.decode, request.deadline).await {
                Ok(mut normalized) => {
                    normalized.at_ms = Some(at_ms);
                    images.push(normalized);
                }
                Err(error) => failure = Some(DeriveError::from(error)),
            }
        }
        match (images.is_empty(), failure) {
            (true, Some(error)) => Err(error),
            _ => Ok(images),
        }
    }
}

impl VisualExtractor for TransformerKeyframes {
    fn kind(&self) -> VisualKind {
        VisualKind::Keyframes
    }

    fn extract<'a>(
        &'a self,
        request: VisualRequest<'a>,
    ) -> BoxFuture<'a, Result<Vec<NormalizedImage>, DeriveError>> {
        Box::pin(async move {
            let error = match self.keyframes(request).await {
                Ok(frames) => return Ok(frames),
                Err(error) => error,
            };
            let has_thumbnail = request
                .target
                .attachment(request.index)
                .is_some_and(|attachment| attachment.thumbnail.is_some());
            if !has_thumbnail {
                return Err(error);
            }
            tracing::info!(
                channel = %telemetry::channel_hash(request.target.channel.id.as_str()),
                reason = error.as_str(),
                "keyframes failed; falling back to the platform thumbnail"
            );
            // The thumbnail's image keeps `at_ms: None`, so it is reported
            // as a platform thumbnail rather than a keyframe.
            self.thumbnail.extract(request).await.map_err(|_| error)
        })
    }
}

/// Every speech-to-text input as 16 kHz mono PCM in a WAV, metered exactly,
/// with the direct upload as the fallback.
pub struct TransformerTranscoder {
    transformer: Arc<dyn MediaTransformer>,
    direct: NoTranscoder,
    transcriber: Arc<dyn Transcriber>,
    fetch_cap: u64,
    seconds: u16,
}

impl TransformerTranscoder {
    /// `max_bytes` is `CMAEG_MEDIA_MAX_BYTES`, the largest original fetched.
    pub fn new(
        transformer: Arc<dyn MediaTransformer>,
        stt: &SttConfig,
        transcriber: Arc<dyn Transcriber>,
        max_bytes: u64,
    ) -> Self {
        Self {
            transformer,
            direct: NoTranscoder::new(stt, transcriber.clone()),
            transcriber,
            fetch_cap: max_bytes,
            seconds: transcode_seconds(stt),
        }
    }
}

/// `T = min(CMAEG_STT_MAX_SECONDS, ⌊(CMAEG_STT_MAX_BYTES − 44) / 32000⌋)`:
/// the longest PCM that is both affordable in one call and uploadable.
pub fn transcode_seconds(stt: &SttConfig) -> u16 {
    let by_bytes =
        stt.max_bytes.saturating_sub(WAV_HEADER_BYTES as u64) / PCM_BYTES_PER_SECOND as u64;
    let seconds = u64::from(stt.max_seconds).min(by_bytes);
    u16::try_from(seconds).unwrap_or(u16::MAX)
}

impl AudioTranscoder for TransformerTranscoder {
    fn fetch_cap(&self) -> u64 {
        self.fetch_cap
    }

    fn plan<'a>(
        &'a self,
        original: Bytes,
        sniffed: Sniffed,
        channel: &'a ChannelId,
        deadline: Instant,
    ) -> BoxFuture<'a, Result<PlannedUpload, DeriveError>> {
        Box::pin(async move {
            // An endpoint configured without WAV cannot take the transcode;
            // the direct path is all that can work.
            if !self.transcriber.accepts(AudioContainer::Wav) {
                return self.direct.plan(original, sniffed, channel, deadline).await;
            }
            let transcoded = timeout_at(
                deadline,
                self.transformer.audio(AudioRequest {
                    channel: channel.clone(),
                    input: original.clone(),
                    seconds: self.seconds,
                }),
            )
            .await
            .unwrap_or(Err(TransformError::job(TransformOutcome::Timeout)));
            let error = match transcoded {
                Ok(pcm) => {
                    return Ok(PlannedUpload {
                        input: AudioInput {
                            bytes: pcm.wav,
                            container: AudioContainer::Wav,
                        },
                        metered_seconds: pcm_seconds(pcm.data_len),
                        truncated_at_seconds: pcm.truncated.then_some(u32::from(self.seconds)),
                    })
                }
                Err(error) => reason(error),
            };
            // An original the direct path accepts (small enough, in a
            // container the endpoint takes) is still transcribed; otherwise
            // the transform's reason, not the direct path's, is reported.
            match self.direct.plan(original, sniffed, channel, deadline).await {
                Ok(planned) => {
                    tracing::info!(
                        channel = %telemetry::channel_hash(channel.as_str()),
                        reason = error.as_str(),
                        "audio transcode failed; uploading the original directly"
                    );
                    Ok(planned)
                }
                Err(_) => Err(error),
            }
        })
    }
}
