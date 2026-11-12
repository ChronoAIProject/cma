//! Observe uninterrupted execution without counting time suspended in the executor.
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::{Duration, Instant},
};

pub(super) trait Clock: Send {
    fn now(&self) -> Instant;
}

pub(super) struct MonotonicClock;
impl Clock for MonotonicClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(super) struct SectionStats {
    pub(super) sections: u64,
    pub(super) longest_section: Duration,
    pub(super) longest_poll: Duration,
}

pub(super) struct ExecutionSections<C> {
    clock: C,
    started: Option<Instant>,
    stats: SectionStats,
}

impl<C: Clock> ExecutionSections<C> {
    pub(super) fn new(clock: C) -> Self {
        let started = Some(clock.now());
        Self {
            clock,
            started,
            stats: SectionStats::default(),
        }
    }

    pub(super) async fn observe<F: Future + Send>(
        &mut self,
        future: F,
        record_poll: bool,
    ) -> F::Output {
        tokio::pin!(future);
        Observed {
            sections: self,
            future: future.as_mut(),
            record_poll,
        }
        .await
    }

    fn suspend(&mut self, now: Instant) {
        if let Some(started) = self.started.take() {
            self.stats.sections = self.stats.sections.saturating_add(1);
            self.stats.longest_section = self
                .stats
                .longest_section
                .max(now.saturating_duration_since(started));
        }
    }

    pub(super) fn finish(&mut self) -> SectionStats {
        self.suspend(self.clock.now());
        self.stats
    }
}

// A named future keeps the borrowed cursor's Send proof local without boxing
// each row or hiding its poll boundary behind another task.
struct Observed<'a, 'f, C, F> {
    sections: &'a mut ExecutionSections<C>,
    future: Pin<&'f mut F>,
    record_poll: bool,
}
impl<C: Clock, F: Future> Future for Observed<'_, '_, C, F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let started = this.sections.clock.now();
        this.sections.started.get_or_insert(started);
        let result = this.future.as_mut().poll(cx);
        let finished = this.sections.clock.now();
        match &result {
            Poll::Pending => this.sections.suspend(finished),
            Poll::Ready(_) if this.record_poll => {
                this.sections.stats.longest_poll = this
                    .sections
                    .stats
                    .longest_poll
                    .max(finished.saturating_duration_since(started));
            }
            Poll::Ready(_) => {}
        }
        result
    }
}

#[cfg(test)]
#[path = "execution_sections_tests.rs"]
mod tests;
