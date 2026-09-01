//! One-minute scheduling for shared-host inactivity pauses.

use std::collections::HashSet;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use async_trait::async_trait;
use layer2_core::store::{Page, PageCursor, PageRequest};
use layer2_core::{ChatHostId, ChatPauseOutcome, CoreError, SessionService};
use tokio::time::MissedTickBehavior;

const PAUSE_SWEEP_INTERVAL: Duration = Duration::from_secs(60);
const PAUSE_PAGE_SIZE: u32 = 200;
const PAUSE_MAX_PAGES_PER_PASS: usize = 8;

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

#[async_trait]
trait PauseBackend: Send + Sync {
    async fn candidates(&self, now: &str, page: PageRequest)
        -> Result<Page<ChatHostId>, CoreError>;

    async fn pause(&self, id: &ChatHostId, now: &str) -> Result<ChatPauseOutcome, CoreError>;
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

struct ChatPauseSweeper {
    backend: Arc<dyn PauseBackend>,
    page_size: u32,
    max_pages: usize,
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

/// Run inactivity maintenance until the process exits.
pub(crate) fn spawn(service: Arc<SessionService>) {
    let sweeper = ChatPauseSweeper::production(service);
    tokio::spawn(run_on_cadence(PAUSE_SWEEP_INTERVAL, move || {
        let sweeper = Arc::clone(&sweeper);
        async move {
            sweeper.run_pass().await;
        }
    }));
    tracing::info!(
        sweep_seconds = PAUSE_SWEEP_INTERVAL.as_secs(),
        page_size = PAUSE_PAGE_SIZE,
        max_pages = PAUSE_MAX_PAGES_PER_PASS,
        "chat inactivity cadence started"
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

    #[test]
    fn cadence_and_page_bounds_are_fixed() {
        assert_eq!(PAUSE_SWEEP_INTERVAL, Duration::from_secs(60));
        assert_eq!(PAUSE_PAGE_SIZE, 200);
        assert_eq!(PAUSE_MAX_PAGES_PER_PASS, 8);
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
