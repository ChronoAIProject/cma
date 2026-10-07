//! The media facade turns and tools depend on.
//!
//! [`MediaService`] is the one injected object that opens a turn's
//! event_ref, charges per-channel quotas, runs single-flight derivation jobs
//! over the source registry and serves the shared response budgets. Callers
//! never see the reply token: they hold a [`MediaTarget`] whose Debug omits
//! it, and every fetch happens inside a job. [`compose`] is the only place
//! that chooses implementations; everything else depends on the seams.
use super::{
    budget::{ChannelLimiter, InflightBytes, JobPermits, ResponseBudget},
    cache::CacheKey,
    cache::DerivedCache,
    derive::{Artifact, DeriveError, Derived, Pipeline},
    jobs::{JobMap, JobOutcome, JOB_CAP},
    source::{
        inline::InlineSource,
        nyxid::NyxidSource,
        telegram::TelegramFileSource,
        url::{PinnedTransport, UrlSource},
        SourceRegistry,
    },
    target::{MediaTarget, TargetError},
    transcribe::{DisabledTranscriber, Transcriber},
    visual::{PlatformThumbnail, VisualExtractor, VisualKind},
    MediaLimits,
};
use crate::{
    ingress::launch::LaunchRequest,
    telegram::TelegramClient,
    telemetry::{self, names},
    turns::render_media::Audience,
    Config, ConfigError, MediaToolProviders,
};
use cmaeg_core::{channel::ChannelDefinition, seal::MasterKeyRing};
use futures_util::future::BoxFuture;
use std::{
    fmt,
    sync::Arc,
    time::{Duration, Instant as StdInstant},
};
use tokio::{sync::Semaphore, time::Instant};
use tracing::{field::Empty, Instrument};

/// The implementations a [`MediaService`] runs on.
pub struct MediaDeps {
    pub sources: Arc<SourceRegistry>,
    pub transcriber: Arc<dyn Transcriber>,
    pub visuals: Arc<dyn VisualExtractor>,
    pub limits: MediaLimits,
    pub keys: Arc<MasterKeyRing>,
    pub tool_providers: Arc<MediaToolProviders>,
    /// One permit, shared with the visual extractor.
    pub decode: Arc<Semaphore>,
}

/// What the configured implementations can produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaCapabilities {
    pub transcriber_enabled: bool,
    pub visuals: VisualKind,
}

/// The single media facade: targets, derivations and shared budgets.
pub struct MediaService {
    pipeline: Arc<Pipeline>,
    keys: Arc<MasterKeyRing>,
    tool_providers: Arc<MediaToolProviders>,
    jobs: JobMap<Derived, DeriveError>,
    permits: JobPermits,
    responses: ResponseBudget,
    limiter: Arc<ChannelLimiter>,
}

/// Only budgets and capabilities: keys and sources stay out of logs.
impl fmt::Debug for MediaService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MediaService")
            .field("limits", &self.pipeline.limits)
            .field("capabilities", &self.capabilities())
            .finish_non_exhaustive()
    }
}

impl MediaService {
    pub fn new(deps: MediaDeps) -> Self {
        let limits = deps.limits;
        let limiter = Arc::new(ChannelLimiter::new(&limits));
        let permits = JobPermits::new(&limits);
        let jobs = JobMap::new(
            DerivedCache::new(limits.cache_bytes),
            permits.clone(),
            limiter.clone(),
        );
        Self {
            pipeline: Arc::new(Pipeline {
                sources: deps.sources,
                transcriber: deps.transcriber,
                visuals: deps.visuals,
                limits,
                decode: deps.decode,
                inflight: InflightBytes::new(&limits),
            }),
            keys: deps.keys,
            tool_providers: deps.tool_providers,
            jobs,
            permits,
            responses: ResponseBudget::new(&limits),
            limiter,
        }
    }

    /// The media target of a turn: its sealed event_ref, opened and checked
    /// against the turn's own channel and activity.
    pub fn target_for_turn(
        &self,
        turn: &LaunchRequest,
        now: i64,
    ) -> Result<MediaTarget, TargetError> {
        let opened = turn
            .event_ref
            .open(&self.keys, now)
            .map_err(|_| TargetError::Unverifiable)?;
        MediaTarget::new(
            turn.channel.clone(),
            opened.payload,
            Arc::new(turn.activity.clone()),
        )
    }

    /// `artifact` of attachment `index`: from the cache, from a running job,
    /// or from a new job, waiting at most `wait` before answering `Pending`.
    ///
    /// Only a new job is charged to the channel's fetch quota.
    pub async fn derive(
        &self,
        target: &MediaTarget,
        index: usize,
        artifact: Artifact,
        wait: Duration,
    ) -> JobOutcome<Derived, DeriveError> {
        let key = match self.admit(target, index, artifact) {
            Ok(Admission::Cached(value)) => return JobOutcome::Ready(value),
            Ok(Admission::Run(key)) => key,
            Err(error) => return JobOutcome::Failed(error),
        };
        let channel = &target.channel.id;
        if !self.jobs.is_live(&key) && !self.limiter.fetch.check(channel) {
            tracing::info!(
                channel = %telemetry::channel_hash(channel.as_str()),
                op = artifact.tag(),
                "media derivation refused: channel fetch quota exhausted"
            );
            return JobOutcome::Failed(DeriveError::RateLimited);
        }
        let job = self.job(target, index, artifact);
        self.jobs.run(key, channel, wait, job).await
    }

    /// Start `artifact` of attachment `index` in the background when a
    /// permit is free, without waiting. A warm-up the channel's quota cannot
    /// afford is skipped.
    pub fn warm(&self, target: &MediaTarget, index: usize, artifact: Artifact) {
        let Ok(Admission::Run(key)) = self.admit(target, index, artifact) else {
            return;
        };
        if self.jobs.is_live(&key) || self.permits.available() == 0 {
            return;
        }
        let channel = &target.channel.id;
        if !self.limiter.fetch.check(channel) {
            tracing::debug!(
                channel = %telemetry::channel_hash(channel.as_str()),
                op = artifact.tag(),
                "media warm-up skipped: channel fetch quota exhausted"
            );
            return;
        }
        let job = self.job(target, index, artifact);
        self.jobs.warm(key, channel, job);
    }

    /// Refusals that need no fetch, then the cache.
    fn admit(
        &self,
        target: &MediaTarget,
        index: usize,
        artifact: Artifact,
    ) -> Result<Admission, DeriveError> {
        let position = u8::try_from(index).map_err(|_| DeriveError::NotFound)?;
        target.attachment(index).ok_or(DeriveError::NotFound)?;
        if artifact == Artifact::Transcript && !self.pipeline.transcriber.enabled() {
            return Err(DeriveError::TranscriberDisabled);
        }
        let key = target.cache_key(position, artifact.tag());
        Ok(match self.jobs.cached(&key) {
            Some(value) => Admission::Cached(value),
            None => Admission::Run(key),
        })
    }

    /// The `'static` job for one derivation. Its deadline starts now, when
    /// the job map spawns it, like the map's own cap.
    fn job(
        &self,
        target: &MediaTarget,
        index: usize,
        artifact: Artifact,
    ) -> impl FnOnce() -> BoxFuture<'static, Result<Derived, DeriveError>> + Send + 'static {
        let pipeline = self.pipeline.clone();
        let target = target.clone();
        let deadline = Instant::now() + JOB_CAP;
        let span = tracing::info_span!(
            "cmaeg.media.derive",
            op = artifact.tag(),
            outcome = Empty,
            channel = %telemetry::channel_hash(target.channel.id.as_str()),
            index,
        );
        move || {
            let record = Record::start(artifact, span.clone());
            Box::pin(
                async move {
                    let result = pipeline.run(&target, index, artifact, deadline).await;
                    record.finish(result.as_ref().err().copied());
                    result
                }
                .instrument(span),
            )
        }
    }

    /// What the agent behind `definition` can do with attachments.
    pub fn audience(&self, definition: &ChannelDefinition) -> Audience {
        Audience::of(definition, &self.tool_providers)
    }

    pub fn capabilities(&self) -> MediaCapabilities {
        MediaCapabilities {
            transcriber_enabled: self.pipeline.transcriber.enabled(),
            visuals: self.pipeline.visuals.kind(),
        }
    }

    /// The pod-wide budget of media response bodies, for tool handlers.
    pub fn responses(&self) -> &ResponseBudget {
        &self.responses
    }

    /// The per-channel quotas, for tool handlers.
    pub fn limiter(&self) -> &ChannelLimiter {
        &self.limiter
    }

    /// The source registry, for tool handlers that stream originals.
    pub fn sources(&self) -> &SourceRegistry {
        &self.pipeline.sources
    }
}

enum Admission {
    Cached(Arc<Derived>),
    Run(CacheKey),
}

/// The duration and outcome of one derivation. A job dropped at its cap
/// never finishes, so dropping an unfinished record reports `timed_out`;
/// a panicking job is reported by the panic hook instead.
struct Record {
    artifact: Artifact,
    span: tracing::Span,
    started: StdInstant,
    finished: bool,
}

impl Record {
    fn start(artifact: Artifact, span: tracing::Span) -> Self {
        Self {
            artifact,
            span,
            started: StdInstant::now(),
            finished: false,
        }
    }

    fn finish(mut self, error: Option<DeriveError>) {
        self.finished = true;
        self.report(error.map_or("ok", |error| error.as_str()));
    }

    fn report(&self, outcome: &'static str) {
        self.span.record("outcome", outcome);
        metrics::histogram!(
            names::MEDIA_DERIVE,
            names::OP => self.artifact.tag(),
            names::OUTCOME => outcome
        )
        .record(self.started.elapsed().as_secs_f64());
        self.span.in_scope(|| {
            if outcome == "ok" {
                tracing::debug!(outcome, "media derivation finished");
            } else {
                tracing::info!(outcome, "media derivation failed");
            }
        });
    }
}

impl Drop for Record {
    fn drop(&mut self) {
        if !self.finished && !std::thread::panicking() {
            self.report(DeriveError::TimedOut.as_str());
        }
    }
}

/// What [`compose`] needs beyond the configuration.
pub struct ComposeDeps {
    pub keys: Arc<MasterKeyRing>,
    pub tool_providers: Arc<MediaToolProviders>,
    pub telegram: Arc<TelegramClient>,
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
    let sources = Arc::new(SourceRegistry::new(
        Arc::new(NyxidSource::new(&config.management.nyxid_base_url)?),
        Arc::new(TelegramFileSource::new(deps.telegram)),
        Arc::new(UrlSource::new(Arc::new(PinnedTransport))),
        Arc::new(InlineSource),
    ));
    let decode = Arc::new(Semaphore::new(1));
    let service = MediaService::new(MediaDeps {
        visuals: Arc::new(PlatformThumbnail::new(sources.clone(), decode.clone())),
        sources,
        transcriber: Arc::new(DisabledTranscriber),
        limits,
        keys: deps.keys,
        tool_providers: deps.tool_providers,
        decode,
    });
    tracing::info!(?limits, capabilities = ?service.capabilities(), "inbound media service composed");
    Ok(Some(Arc::new(service)))
}
