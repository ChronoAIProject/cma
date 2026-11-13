//! Event-driven tailing with a fixed fallback and current release authority.
use super::{cursor, error::Error, stream::Following};
use crate::stream_telemetry::{self, AgentWake, Kind, Poll};
use std::future::pending;
use tokio::{sync::broadcast::error::RecvError, time::Instant};

impl Following {
    pub(super) async fn wait_for_change(&mut self) -> Result<(), Error> {
        // Buffered live data resumes immediately after its durable predecessor.
        if !self.initial_gap
            && self.deferred_live.is_empty()
            && self
                .held
                .as_ref()
                .is_none_or(|held| held.after_seq > self.header.head)
        {
            loop {
                let deadline = self.authorization.revalidation_deadline();
                let allow_live = self.held.is_none();
                tokio::select! {
                    biased;
                    _ = async {
                        if let Some(deadline) = deadline { tokio::time::sleep_until(deadline).await; }
                        else { pending::<()>().await; }
                    } => { stream_telemetry::agent_wake(AgentWake::Authorization); break; }
                    _ = tokio::time::sleep_until(self.fallback_due) => {
                        self.fallback_due = Instant::now() + self.state.fallback_poll;
                        stream_telemetry::agent_wake(AgentWake::Fallback);
                        break;
                    }
                    received = async {
                        match &mut self.heads { Some(heads) => heads.recv().await, None => pending().await }
                    } => {
                        if received.is_err() { self.heads = None; continue; }
                        stream_telemetry::agent_wake(AgentWake::Head);
                        break;
                    }
                    received = async {
                        if !allow_live { return pending().await; }
                        match &mut self.watching { Some(live) => live.recv().await, None => pending().await }
                    } => {
                        match received {
                            Ok(fragment) => self.held = Some(fragment),
                            Err(RecvError::Lagged(count)) => self.discarded_fragments += count,
                            Err(RecvError::Closed) => { self.watching = None; continue; }
                        }
                        stream_telemetry::agent_wake(AgentWake::Live);
                        break;
                    }
                }
            }
        }
        let poll = Poll::new(Kind::AgentEventsV2);
        let result = self.refresh_tail().await;
        if result.is_ok() {
            poll.complete(!self.pending.is_empty());
        }
        result
    }

    async fn refresh_tail(&mut self) -> Result<(), Error> {
        let current = self
            .state
            .reads
            .current_event_stream_header(&self.authorization)
            .await?;
        if cursor::incarnation(&current) != self.identity {
            return Err(Error::Incarnation);
        }
        if current != self.header || self.after < current.head {
            self.header = current;
            // A hint is never replay authority. Keep the existing page and tail
            // proofs, plus per-durable-frame release checks, after head changes.
            return Box::pin(self.fill()).await;
        }
        self.queue_tail();
        // This snapshot is the live batch's release fence. No IO occurs between
        // it, queuing, and the first frame leaving this poll of the response.
        if self
            .pending
            .front()
            .is_some_and(|frame| frame.fragment.is_some())
        {
            self.live_batch_needs_validation = false;
        }
        Ok(())
    }
}
