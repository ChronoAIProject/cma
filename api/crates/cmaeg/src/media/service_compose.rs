//! Compose one shared transformer for inbound media and outbound voice metadata.
use super::{MediaDeps, MediaService};
use crate::{
    media::{
        budget::InflightBytes,
        source::{
            inline::InlineSource,
            nyxid::NyxidSource,
            telegram::TelegramFileSource,
            url::{PinnedTransport, UrlSource},
            SourceRegistry,
        },
        transcribe::{
            meter::SttSpend,
            openai::OpenAiCompatibleTranscriber,
            upload::{AudioTranscoder, NoTranscoder},
            DisabledTranscriber, Transcriber,
        },
        transform::{
            adapters::{TransformerKeyframes, TransformerTranscoder},
            ffmpeg::FfmpegJobTransformer,
            scheduler::TransformScheduler,
            setup::TransformerRuntime,
            MediaTransformer,
        },
        visual::{PlatformThumbnail, VisualExtractor},
        MediaLimits,
    },
    telegram::TelegramClient,
    Config, ConfigError, MediaToolProviders,
};
use cmaeg_core::seal::MasterKeyRing;
use std::sync::Arc;
use tokio::sync::Semaphore;

/// What [`compose`] needs beyond the configuration.
pub struct ComposeDeps {
    pub resolver: crate::channels::resolver::ChannelResolver,
    pub keys: Arc<MasterKeyRing>,
    pub tool_providers: Arc<MediaToolProviders>,
    pub telegram: Arc<TelegramClient>,
    /// The media transformer's launcher and root once its startup setup
    /// passed; `None` while it is not configured.
    pub transformer: Option<TransformerRuntime>,
}

/// The media service, or `None` while media is off, so that no budget or
/// cache is allocated and the pod's memory needs do not change.
///
/// This is the only place that chooses implementations.
pub fn compose(
    config: &Config,
    deps: ComposeDeps,
) -> Result<Option<Arc<MediaService>>, ConfigError> {
    if !config.media.enabled {
        tracing::debug!("inbound media is disabled; no media service composed");
        return Ok(None);
    }
    let limits = MediaLimits::from_config(&config.media);
    let sources = Arc::new(
        SourceRegistry::new(
            Arc::new(
                NyxidSource::new(&config.management.nyxid_base_url)?
                    .with_resolver(deps.resolver.clone()),
            ),
            Arc::new(TelegramFileSource::new(deps.telegram)),
            Arc::new(UrlSource::new(Arc::new(PinnedTransport))),
            Arc::new(InlineSource),
        )
        .with_discord(Arc::new(
            crate::media::source::discord::DiscordFileSource::new(
                crate::discord::DiscordRest::new()?,
            ),
        ))
        .with_whatsapp(Arc::new(
            crate::media::source::whatsapp::WhatsappFileSource::new(),
        ))
        .with_native_lark(Arc::new(
            crate::media::source::native_lark::NativeLarkFileSource::new(
                crate::native_lark::Client::production()?,
                deps.resolver.clone(),
            ),
        )),
    );
    let decode = Arc::new(Semaphore::new(1));
    // An empty CMAEG_STT_URL leaves speech-to-text off.
    let transcriber: Arc<dyn Transcriber> = match &config.stt.endpoint {
        Some(endpoint) => Arc::new(OpenAiCompatibleTranscriber::new(endpoint, &config.stt)?),
        None => Arc::new(DisabledTranscriber),
    };
    let inflight = InflightBytes::new(&limits);
    // Without the transformer, platform thumbnails and the direct upload
    // path; with it, keyframes and PCM, each keeping the former as fallback.
    let mut voice_decoder = None;
    let (visuals, transcoder): (Arc<dyn VisualExtractor>, Arc<dyn AudioTranscoder>) =
        match deps.transformer {
            None => (
                Arc::new(PlatformThumbnail::new(sources.clone(), decode.clone())),
                Arc::new(NoTranscoder::new(&config.stt, transcriber.clone())),
            ),
            Some(runtime) => {
                let transformer: Arc<dyn MediaTransformer> = Arc::new(FfmpegJobTransformer::new(
                    runtime.launcher,
                    runtime.root,
                    TransformScheduler::new(),
                    inflight.clone(),
                    config.transformer.video_frames,
                ));
                voice_decoder = Some(crate::discord::voice::Decoder::new(
                    transformer.clone(),
                    inflight.clone(),
                    limits.inflight_bytes,
                ));
                (
                    Arc::new(TransformerKeyframes::new(
                        transformer.clone(),
                        sources.clone(),
                        decode.clone(),
                        limits.max_bytes,
                        config.transformer.video_frames,
                    )),
                    Arc::new(TransformerTranscoder::new(
                        transformer,
                        &config.stt,
                        transcriber.clone(),
                        limits.max_bytes,
                    )),
                )
            }
        };
    let service = MediaService::new(MediaDeps {
        visuals,
        sources,
        transcoder,
        stt: Arc::new(SttSpend::new(&config.stt)),
        transcriber,
        limits,
        keys: deps.keys,
        tool_providers: deps.tool_providers,
        decode,
        inflight,
    })
    .with_resolver(deps.resolver)
    .with_voice_decoder(voice_decoder);
    tracing::info!(?limits, capabilities = ?service.capabilities(), "inbound media service composed");
    Ok(Some(Arc::new(service)))
}
