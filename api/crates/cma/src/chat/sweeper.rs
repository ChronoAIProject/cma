//! One-minute scheduling for shared-host image rotation and inactivity pauses.

use std::collections::HashSet;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use async_trait::async_trait;
use futures_util::StreamExt as _;
use layer2_core::store::{Page, PageCursor, PageRequest};
use layer2_core::{
    ChatHostId, ChatPauseOutcome, ChatTitleGenerator, ChatTitleOutcome, CoreError, SessionId,
    SessionService,
};
use tokio::time::MissedTickBehavior;

const PAUSE_SWEEP_INTERVAL: Duration = Duration::from_secs(60);
const PAUSE_PAGE_SIZE: u32 = 200;
const PAUSE_MAX_PAGES_PER_PASS: usize = 8;
const ROTATION_PAGE_SIZE: u32 = 200;
const ROTATION_MAX_PAGES_PER_PASS: usize = 8;
const TITLE_PAGE_SIZE: u32 = 16;
const TITLE_CONCURRENCY: usize = 8;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct PausePassStats {
    pages: usize,
    candidates: usize,
    paused: usize,
    skipped: usize,
    refused: usize,
    errors: usize,
    page_budget_exhausted: bool,
    elapsed_milliseconds: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct RotationPassStats {
    pages: usize,
    candidates: usize,
    rotated: usize,
    skipped: usize,
    errors: usize,
    page_budget_exhausted: bool,
    elapsed_milliseconds: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct TitlePassStats {
    pages: usize,
    candidates: usize,
    generated: usize,
    fallbacks: usize,
    skipped: usize,
    errors: usize,
    page_budget_exhausted: bool,
    elapsed_milliseconds: u64,
}

#[async_trait]
trait PauseBackend: Send + Sync {
    async fn candidates(&self, now: &str, page: PageRequest)
        -> Result<Page<ChatHostId>, CoreError>;

    async fn pause(&self, id: &ChatHostId, now: &str) -> Result<ChatPauseOutcome, CoreError>;
}

#[async_trait]
trait RotationBackend: Send + Sync {
    async fn candidates(&self, page: PageRequest) -> Result<Page<ChatHostId>, CoreError>;

    async fn rotate(&self, id: &ChatHostId) -> Result<bool, CoreError>;
}

#[async_trait]
trait TitleBackend: Send + Sync {
    async fn candidates(&self, page: PageRequest) -> Result<Page<SessionId>, CoreError>;

    async fn generate(&self, id: &SessionId) -> Result<ChatTitleOutcome, CoreError>;
}

#[async_trait]
impl PauseBackend for SessionService {
    async fn candidates(
        &self,
        now: &str,
        page: PageRequest,
    ) -> Result<Page<ChatHostId>, CoreError> {
        self.chat_pause_candidates(now, page)
            .await
            .map(|page| page.map(|host| host.id))
    }

    async fn pause(&self, id: &ChatHostId, now: &str) -> Result<ChatPauseOutcome, CoreError> {
        self.pause_chat_host(id, now).await
    }
}

struct SessionRotationBackend(Arc<SessionService>);

struct SessionTitleBackend {
    service: Arc<SessionService>,
    generator: Option<Arc<dyn ChatTitleGenerator>>,
}

#[async_trait]
impl RotationBackend for SessionRotationBackend {
    async fn candidates(&self, page: PageRequest) -> Result<Page<ChatHostId>, CoreError> {
        self.0
            .chat_rotation_candidates(page)
            .await
            .map(|page| page.map(|host| host.id))
    }

    async fn rotate(&self, id: &ChatHostId) -> Result<bool, CoreError> {
        self.0.rotate_chat_host_on_image_drift(id).await
    }
}

#[async_trait]
impl TitleBackend for SessionTitleBackend {
    async fn candidates(&self, page: PageRequest) -> Result<Page<SessionId>, CoreError> {
        self.service.chat_title_candidates(page).await
    }

    async fn generate(&self, id: &SessionId) -> Result<ChatTitleOutcome, CoreError> {
        self.service
            .generate_chat_title(id, self.generator.as_deref())
            .await
    }
}

struct ChatPauseSweeper {
    backend: Arc<dyn PauseBackend>,
    page_size: u32,
    max_pages: usize,
    resume_cursor: Mutex<Option<PageCursor>>,
}

struct ChatRotationSweeper {
    backend: Arc<dyn RotationBackend>,
    page_size: u32,
    max_pages: usize,
    resume_cursor: Mutex<Option<PageCursor>>,
}

struct ChatTitleSweeper {
    backend: Arc<dyn TitleBackend>,
    page_size: u32,
    concurrency: usize,
    resume_cursor: Mutex<Option<PageCursor>>,
}

impl ChatPauseSweeper {
    fn production(service: Arc<SessionService>) -> Arc<Self> {
        Arc::new(Self {
            backend: service,
            page_size: PAUSE_PAGE_SIZE,
            max_pages: PAUSE_MAX_PAGES_PER_PASS,
            resume_cursor: Mutex::new(None),
        })
    }

    async fn run_pass(&self) -> PausePassStats {
        self.run_pass_at(&layer2_core::rfc3339_at(SystemTime::now()))
            .await
    }

    async fn run_pass_at(&self, now: &str) -> PausePassStats {
        let started = Instant::now();
        let mut stats = PausePassStats::default();
        let mut cursor = self
            .resume_cursor
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        let mut next_pass_cursor = cursor.clone();
        let mut seen = HashSet::new();

        for page_index in 0..self.max_pages {
            let page = match self
                .backend
                .candidates(now, PageRequest::new(self.page_size, cursor.clone()))
                .await
            {
                Ok(page) => page,
                Err(error) => {
                    stats.errors += 1;
                    tracing::warn!(%error, "chat inactivity candidate page could not be read");
                    break;
                }
            };
            stats.pages += 1;
            stats.candidates += page.items.len();
            for host_id in page.items {
                match self.backend.pause(&host_id, now).await {
                    Ok(ChatPauseOutcome::Complete(_)) => stats.paused += 1,
                    Ok(ChatPauseOutcome::Refused) => stats.refused += 1,
                    Ok(ChatPauseOutcome::NotEligible | ChatPauseOutcome::Busy) => {
                        stats.skipped += 1;
                    }
                    Err(error) => {
                        stats.errors += 1;
                        tracing::warn!(chat_host = %host_id, %error, "chat inactivity candidate failed");
                    }
                }
            }

            let Some(next) = page.next else {
                next_pass_cursor = None;
                break;
            };
            if !seen.insert(next.clone()) {
                stats.errors += 1;
                next_pass_cursor = None;
                tracing::error!("chat inactivity sweep stopped a repeated page cursor");
                break;
            }
            next_pass_cursor = Some(next.clone());
            if page_index + 1 == self.max_pages {
                stats.page_budget_exhausted = true;
                break;
            }
            cursor = Some(next);
        }

        *self
            .resume_cursor
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = next_pass_cursor;
        stats.elapsed_milliseconds = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
        log_pass(stats);
        stats
    }
}

impl ChatRotationSweeper {
    fn production(service: Arc<SessionService>) -> Arc<Self> {
        Arc::new(Self {
            backend: Arc::new(SessionRotationBackend(service)),
            page_size: ROTATION_PAGE_SIZE,
            max_pages: ROTATION_MAX_PAGES_PER_PASS,
            resume_cursor: Mutex::new(None),
        })
    }

    async fn run_pass(&self) -> RotationPassStats {
        let started = Instant::now();
        let mut stats = RotationPassStats::default();
        let mut cursor = self
            .resume_cursor
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        let mut next_pass_cursor = cursor.clone();
        let mut seen = HashSet::new();

        for page_index in 0..self.max_pages {
            let page = match self
                .backend
                .candidates(PageRequest::new(self.page_size, cursor.clone()))
                .await
            {
                Ok(page) => page,
                Err(error) => {
                    stats.errors += 1;
                    tracing::warn!(%error, "chat rotation candidate page could not be read");
                    break;
                }
            };
            stats.pages += 1;
            stats.candidates += page.items.len();
            for host_id in page.items {
                match self.backend.rotate(&host_id).await {
                    Ok(true) => stats.rotated += 1,
                    Ok(false) => stats.skipped += 1,
                    Err(error) => {
                        stats.errors += 1;
                        tracing::warn!(chat_host = %host_id, %error, "chat image rotation failed");
                    }
                }
            }

            let Some(next) = page.next else {
                next_pass_cursor = None;
                break;
            };
            if !seen.insert(next.clone()) {
                stats.errors += 1;
                next_pass_cursor = None;
                tracing::error!("chat rotation sweep stopped a repeated page cursor");
                break;
            }
            next_pass_cursor = Some(next.clone());
            if page_index + 1 == self.max_pages {
                stats.page_budget_exhausted = true;
                break;
            }
            cursor = Some(next);
        }

        *self
            .resume_cursor
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = next_pass_cursor;
        stats.elapsed_milliseconds = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
        log_rotation_pass(stats);
        stats
    }
}

impl ChatTitleSweeper {
    fn production(
        service: Arc<SessionService>,
        generator: Option<Arc<dyn ChatTitleGenerator>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            backend: Arc::new(SessionTitleBackend { service, generator }),
            page_size: TITLE_PAGE_SIZE,
            concurrency: TITLE_CONCURRENCY,
            resume_cursor: Mutex::new(None),
        })
    }

    async fn run_pass(&self) -> TitlePassStats {
        let started = Instant::now();
        let mut stats = TitlePassStats::default();
        let cursor = self
            .resume_cursor
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        let page = match self
            .backend
            .candidates(PageRequest::new(self.page_size, cursor.clone()))
            .await
        {
            Ok(page) => page,
            Err(error) => {
                stats.errors = 1;
                stats.elapsed_milliseconds =
                    started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
                tracing::warn!(%error, "chat title candidate page could not be read");
                log_title_pass(stats);
                return stats;
            }
        };
        stats.pages = 1;
        stats.candidates = page.items.len();

        let outcomes = futures_util::stream::iter(page.items.into_iter().map(|id| {
            let backend = Arc::clone(&self.backend);
            async move {
                let outcome = backend.generate(&id).await;
                (id, outcome)
            }
        }))
        .buffer_unordered(self.concurrency)
        .collect::<Vec<_>>()
        .await;
        for (id, outcome) in outcomes {
            match outcome {
                Ok(ChatTitleOutcome::Generated) => stats.generated += 1,
                Ok(ChatTitleOutcome::Fallback) => stats.fallbacks += 1,
                Ok(ChatTitleOutcome::Skipped) => stats.skipped += 1,
                Err(error) => {
                    stats.errors += 1;
                    tracing::warn!(session_id = %id, %error, "chat title candidate failed");
                }
            }
        }

        let next = match page.next {
            Some(next) if cursor.as_ref() == Some(&next) => {
                stats.errors += 1;
                tracing::error!("chat title sweep stopped a repeated page cursor");
                None
            }
            next => next,
        };
        stats.page_budget_exhausted = next.is_some();
        *self
            .resume_cursor
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = next;
        stats.elapsed_milliseconds = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
        log_title_pass(stats);
        stats
    }
}

fn log_pass(stats: PausePassStats) {
    let actionable =
        stats.paused > 0 || stats.refused > 0 || stats.errors > 0 || stats.page_budget_exhausted;
    tracing::info!(
        event_name = "cma_chat_inactivity_pass",
        pages = stats.pages,
        candidates = stats.candidates,
        paused = stats.paused,
        skipped = stats.skipped,
        refused = stats.refused,
        errors = stats.errors,
        page_budget_exhausted = stats.page_budget_exhausted,
        elapsed_milliseconds = stats.elapsed_milliseconds,
        actionable,
        "chat inactivity sweep settled"
    );
    if stats.refused > 0 || stats.errors > 0 || stats.page_budget_exhausted {
        tracing::warn!(
            event_name = "cma_chat_inactivity_alert_candidate",
            refused = stats.refused,
            errors = stats.errors,
            page_budget_exhausted = stats.page_budget_exhausted,
            "chat inactivity maintenance needs operator attention"
        );
    }
}

fn log_rotation_pass(stats: RotationPassStats) {
    let actionable = stats.rotated > 0 || stats.errors > 0 || stats.page_budget_exhausted;
    tracing::info!(
        event_name = "cma_chat_rotation_pass",
        pages = stats.pages,
        candidates = stats.candidates,
        rotated = stats.rotated,
        skipped = stats.skipped,
        errors = stats.errors,
        page_budget_exhausted = stats.page_budget_exhausted,
        elapsed_milliseconds = stats.elapsed_milliseconds,
        actionable,
        "chat image-rotation sweep settled"
    );
    if stats.errors > 0 || stats.page_budget_exhausted {
        tracing::warn!(
            event_name = "cma_chat_rotation_alert_candidate",
            errors = stats.errors,
            page_budget_exhausted = stats.page_budget_exhausted,
            "chat image-rotation maintenance needs operator attention"
        );
    }
}

fn log_title_pass(stats: TitlePassStats) {
    let actionable = stats.generated > 0
        || stats.fallbacks > 0
        || stats.errors > 0
        || stats.page_budget_exhausted;
    tracing::info!(
        event_name = "cma_chat_title_pass",
        pages = stats.pages,
        candidates = stats.candidates,
        generated = stats.generated,
        fallbacks = stats.fallbacks,
        skipped = stats.skipped,
        errors = stats.errors,
        page_budget_exhausted = stats.page_budget_exhausted,
        elapsed_milliseconds = stats.elapsed_milliseconds,
        actionable,
        "chat title sweep settled"
    );
    if stats.errors > 0 || stats.page_budget_exhausted {
        tracing::warn!(
            event_name = "cma_chat_title_alert_candidate",
            errors = stats.errors,
            page_budget_exhausted = stats.page_budget_exhausted,
            "chat title maintenance needs operator attention"
        );
    }
}

/// Run chat maintenance until the process exits.
pub(crate) fn spawn(
    service: Arc<SessionService>,
    title_generator: Option<Arc<dyn ChatTitleGenerator>>,
) {
    let pause_sweeper = ChatPauseSweeper::production(Arc::clone(&service));
    let title_sweeper = ChatTitleSweeper::production(Arc::clone(&service), title_generator);
    let rotation_sweeper = ChatRotationSweeper::production(service);
    tokio::spawn(run_on_cadence(PAUSE_SWEEP_INTERVAL, move || {
        let pause_sweeper = Arc::clone(&pause_sweeper);
        let title_sweeper = Arc::clone(&title_sweeper);
        async move {
            tokio::join!(pause_sweeper.run_pass(), title_sweeper.run_pass());
        }
    }));
    tokio::spawn(run_on_cadence(PAUSE_SWEEP_INTERVAL, move || {
        let rotation_sweeper = Arc::clone(&rotation_sweeper);
        async move {
            rotation_sweeper.run_pass().await;
        }
    }));
    tracing::info!(
        sweep_seconds = PAUSE_SWEEP_INTERVAL.as_secs(),
        pause_page_size = PAUSE_PAGE_SIZE,
        pause_max_pages = PAUSE_MAX_PAGES_PER_PASS,
        rotation_page_size = ROTATION_PAGE_SIZE,
        rotation_max_pages = ROTATION_MAX_PAGES_PER_PASS,
        title_page_size = TITLE_PAGE_SIZE,
        title_concurrency = TITLE_CONCURRENCY,
        "chat lifecycle cadence started"
    );
}

async fn run_on_cadence<F, Fut>(interval: Duration, mut sweep: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = ()>,
{
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        sweep().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Mutex as AsyncMutex;

    struct FakeBackend {
        pages: AsyncMutex<VecDeque<Page<ChatHostId>>>,
        calls: AsyncMutex<Vec<Option<PageCursor>>>,
        outcome: ChatPauseOutcome,
        pauses: AtomicUsize,
    }

    struct FakeRotationBackend {
        pages: AsyncMutex<VecDeque<Page<ChatHostId>>>,
        calls: AsyncMutex<Vec<Option<PageCursor>>>,
        outcomes: AsyncMutex<VecDeque<Result<bool, CoreError>>>,
    }

    struct FakeTitleBackend {
        pages: AsyncMutex<VecDeque<Page<SessionId>>>,
        calls: AsyncMutex<Vec<Option<PageCursor>>>,
        outcomes: AsyncMutex<VecDeque<Result<ChatTitleOutcome, CoreError>>>,
    }

    #[async_trait]
    impl PauseBackend for FakeBackend {
        async fn candidates(
            &self,
            _now: &str,
            page: PageRequest,
        ) -> Result<Page<ChatHostId>, CoreError> {
            self.calls.lock().await.push(page.cursor().cloned());
            self.pages
                .lock()
                .await
                .pop_front()
                .ok_or_else(|| CoreError::RuntimeUnavailable("no fake page".into()))
        }

        async fn pause(&self, _id: &ChatHostId, _now: &str) -> Result<ChatPauseOutcome, CoreError> {
            self.pauses.fetch_add(1, Ordering::SeqCst);
            Ok(self.outcome.clone())
        }
    }

    #[async_trait]
    impl RotationBackend for FakeRotationBackend {
        async fn candidates(&self, page: PageRequest) -> Result<Page<ChatHostId>, CoreError> {
            self.calls.lock().await.push(page.cursor().cloned());
            self.pages
                .lock()
                .await
                .pop_front()
                .ok_or_else(|| CoreError::RuntimeUnavailable("no fake rotation page".into()))
        }

        async fn rotate(&self, _id: &ChatHostId) -> Result<bool, CoreError> {
            self.outcomes
                .lock()
                .await
                .pop_front()
                .ok_or_else(|| CoreError::RuntimeUnavailable("no fake rotation outcome".into()))?
        }
    }

    #[async_trait]
    impl TitleBackend for FakeTitleBackend {
        async fn candidates(&self, page: PageRequest) -> Result<Page<SessionId>, CoreError> {
            self.calls.lock().await.push(page.cursor().cloned());
            self.pages
                .lock()
                .await
                .pop_front()
                .ok_or_else(|| CoreError::RuntimeUnavailable("no fake title page".into()))
        }

        async fn generate(&self, _id: &SessionId) -> Result<ChatTitleOutcome, CoreError> {
            self.outcomes
                .lock()
                .await
                .pop_front()
                .ok_or_else(|| CoreError::RuntimeUnavailable("no fake title outcome".into()))?
        }
    }

    fn cursor(id: &str) -> PageCursor {
        PageCursor::new("2026-09-01T00:00:00Z", id)
    }

    #[tokio::test]
    async fn pass_walks_through_an_empty_filtered_page() {
        let next = cursor("chho_next");
        let backend = Arc::new(FakeBackend {
            pages: AsyncMutex::new(VecDeque::from([
                Page {
                    items: Vec::new(),
                    next: Some(next.clone()),
                },
                Page {
                    items: vec![ChatHostId("chho_quiet".into())],
                    next: None,
                },
            ])),
            calls: AsyncMutex::new(Vec::new()),
            outcome: ChatPauseOutcome::Complete(test_checkpoint()),
            pauses: AtomicUsize::new(0),
        });
        let sweeper = ChatPauseSweeper {
            backend: backend.clone(),
            page_size: 1,
            max_pages: 2,
            resume_cursor: Mutex::new(None),
        };

        let stats = sweeper.run_pass_at("2026-09-01T02:00:01Z").await;
        assert_eq!(stats.pages, 2);
        assert_eq!(stats.candidates, 1);
        assert_eq!(stats.paused, 1);
        assert_eq!(backend.pauses.load(Ordering::SeqCst), 1);
        assert_eq!(*backend.calls.lock().await, [None, Some(next)]);
    }

    #[tokio::test]
    async fn page_budget_resumes_after_the_last_scanned_cursor() {
        let next = cursor("chho_resume");
        let backend = Arc::new(FakeBackend {
            pages: AsyncMutex::new(VecDeque::from([
                Page {
                    items: Vec::new(),
                    next: Some(next.clone()),
                },
                Page {
                    items: Vec::new(),
                    next: None,
                },
            ])),
            calls: AsyncMutex::new(Vec::new()),
            outcome: ChatPauseOutcome::NotEligible,
            pauses: AtomicUsize::new(0),
        });
        let sweeper = ChatPauseSweeper {
            backend: backend.clone(),
            page_size: 1,
            max_pages: 1,
            resume_cursor: Mutex::new(None),
        };

        assert!(
            sweeper
                .run_pass_at("2026-09-01T02:00:01Z")
                .await
                .page_budget_exhausted
        );
        assert!(
            !sweeper
                .run_pass_at("2026-09-01T02:01:01Z")
                .await
                .page_budget_exhausted
        );
        assert_eq!(*backend.calls.lock().await, [None, Some(next)]);
    }

    #[tokio::test]
    async fn rotation_pass_walks_filtered_pages_and_types_each_outcome() {
        let next = cursor("chho_rotation_next");
        let backend = Arc::new(FakeRotationBackend {
            pages: AsyncMutex::new(VecDeque::from([
                Page {
                    items: Vec::new(),
                    next: Some(next.clone()),
                },
                Page {
                    items: vec![
                        ChatHostId("chho_rotated".into()),
                        ChatHostId("chho_busy".into()),
                        ChatHostId("chho_failed".into()),
                    ],
                    next: None,
                },
            ])),
            calls: AsyncMutex::new(Vec::new()),
            outcomes: AsyncMutex::new(VecDeque::from([
                Ok(true),
                Ok(false),
                Err(CoreError::RuntimeUnavailable("rotation failed".into())),
            ])),
        });
        let sweeper = ChatRotationSweeper {
            backend: backend.clone(),
            page_size: 2,
            max_pages: 2,
            resume_cursor: Mutex::new(None),
        };

        let stats = sweeper.run_pass().await;

        assert_eq!(stats.pages, 2);
        assert_eq!(stats.candidates, 3);
        assert_eq!(stats.rotated, 1);
        assert_eq!(stats.skipped, 1);
        assert_eq!(stats.errors, 1);
        assert!(!stats.page_budget_exhausted);
        assert_eq!(*backend.calls.lock().await, [None, Some(next)]);
    }

    #[tokio::test]
    async fn title_pass_is_one_bounded_page_and_types_every_content_free_outcome() {
        let next = cursor("sesn_title_next");
        let backend = Arc::new(FakeTitleBackend {
            pages: AsyncMutex::new(VecDeque::from([Page {
                items: ["generated", "fallback", "skipped", "failed"]
                    .into_iter()
                    .map(|suffix| SessionId(format!("sesn_{suffix}")))
                    .collect(),
                next: Some(next.clone()),
            }])),
            calls: AsyncMutex::new(Vec::new()),
            outcomes: AsyncMutex::new(VecDeque::from([
                Ok(ChatTitleOutcome::Generated),
                Ok(ChatTitleOutcome::Fallback),
                Ok(ChatTitleOutcome::Skipped),
                Err(CoreError::RuntimeUnavailable("title call failed".into())),
            ])),
        });
        let sweeper = ChatTitleSweeper {
            backend: backend.clone(),
            page_size: 4,
            concurrency: 2,
            resume_cursor: Mutex::new(None),
        };

        let stats = sweeper.run_pass().await;

        assert_eq!(stats.pages, 1);
        assert_eq!(stats.candidates, 4);
        assert_eq!(stats.generated, 1);
        assert_eq!(stats.fallbacks, 1);
        assert_eq!(stats.skipped, 1);
        assert_eq!(stats.errors, 1);
        assert!(stats.page_budget_exhausted);
        assert_eq!(*backend.calls.lock().await, [None]);
        assert_eq!(
            *sweeper.resume_cursor.lock().unwrap(),
            Some(next),
            "the raw-page cursor is retained even when candidate outcomes differ"
        );
    }

    #[tokio::test]
    async fn title_pass_resumes_through_an_empty_raw_page_then_wraps() {
        let next = cursor("sesn_title_resume");
        let backend = Arc::new(FakeTitleBackend {
            pages: AsyncMutex::new(VecDeque::from([
                Page {
                    items: Vec::new(),
                    next: Some(next.clone()),
                },
                Page {
                    items: Vec::new(),
                    next: None,
                },
            ])),
            calls: AsyncMutex::new(Vec::new()),
            outcomes: AsyncMutex::new(VecDeque::new()),
        });
        let sweeper = ChatTitleSweeper {
            backend: backend.clone(),
            page_size: 1,
            concurrency: 1,
            resume_cursor: Mutex::new(None),
        };

        assert!(sweeper.run_pass().await.page_budget_exhausted);
        assert!(!sweeper.run_pass().await.page_budget_exhausted);
        assert_eq!(*backend.calls.lock().await, [None, Some(next)]);
        assert_eq!(*sweeper.resume_cursor.lock().unwrap(), None);
    }

    #[test]
    fn cadence_and_page_bounds_are_fixed() {
        assert_eq!(PAUSE_SWEEP_INTERVAL, Duration::from_secs(60));
        assert_eq!(PAUSE_PAGE_SIZE, 200);
        assert_eq!(PAUSE_MAX_PAGES_PER_PASS, 8);
        assert_eq!(ROTATION_PAGE_SIZE, 200);
        assert_eq!(ROTATION_MAX_PAGES_PER_PASS, 8);
        assert_eq!(TITLE_PAGE_SIZE, 16);
        assert_eq!(TITLE_CONCURRENCY, 8);
    }

    #[tokio::test(start_paused = true)]
    async fn cadence_ticks_immediately_then_skips_missed_minutes() {
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&calls);
        let task = tokio::spawn(run_on_cadence(PAUSE_SWEEP_INTERVAL, move || {
            observed.fetch_add(1, Ordering::SeqCst);
            std::future::ready(())
        }));
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        tokio::time::advance(Duration::from_secs(5 * 60)).await;
        for _ in 0..4 {
            tokio::task::yield_now().await;
        }
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        task.abort();
    }

    fn test_checkpoint() -> layer2_core::ChatCheckpointRef {
        // The scheduler treats the reference as an opaque success receipt;
        // sealing and cryptographic validation are covered in Layer 2.
        layer2_core::ChatCheckpointRef {
            workspace_object_reference: "objects/workspace".into(),
            workspace_archive_sha256: "a".repeat(64),
            workspace_manifest_digest: "b".repeat(64),
            conversations: vec![layer2_core::ChatCheckpointConversationRef {
                session_id: layer2_core::SessionId("sesn_scheduler".into()),
                object_reference: "objects/conversation".into(),
                sha256: "c".repeat(64),
                conversation_id: "conversation-scheduler".into(),
            }],
            captured_at: "2026-09-01T02:00:01Z".into(),
            checkpoint_version: 1,
            pair_sha256: "d".repeat(64),
        }
    }
}
