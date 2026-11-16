use super::*;
use layer2_core::sandbox::{AgentEventStreamHeader, AgentEventStreamSnapshot, AgentReadError};
use std::future::pending;

impl Following {
    async fn header(&self) -> Result<AgentEventStreamHeader, AgentReadError> {
        if let Some(authorization) = &self.authorization {
            self.state
                .reads
                .current_event_stream_header(authorization)
                .await
        } else {
            let agent = self.state.reads.read(&self.caller, &self.id).await?;
            Ok(AgentEventStreamHeader {
                id: agent.id,
                created_at: agent.created_at,
                runtime_generation: agent.conversation.runtime_generation,
                active_turn: agent.conversation.active_turn,
                head: agent.conversation.last_seq,
            })
        }
    }

    async fn page(
        &mut self,
        through: Option<u64>,
    ) -> Result<AgentEventStreamSnapshot, AgentReadError> {
        self.observed_at = tokio::time::Instant::now();
        if let Some(authorization) = &self.authorization {
            self.state
                .reads
                .event_stream_page(authorization, &mut self.cursor, through)
                .await
        } else {
            self.state
                .reads
                .owner_event_stream_page(&self.caller, &self.id, &mut self.cursor, through)
                .await
        }
    }

    pub(super) async fn initialize(&mut self) -> Result<(), AgentReadError> {
        let header = self.observe(true).await?;
        if self.after > header.head {
            self.header = Some(header);
            return Err(AgentReadError::InvalidPage);
        }
        self.through = (!self.follow).then_some(header.head);
        self.header = Some(header);
        let prefix = self.after;
        // Reconstruct compact context before the retained cursor in bounded pages.
        while self.cursor.sequence() < prefix {
            let page = self.page(Some(prefix)).await?;
            for entry in page.history {
                if let Some(kind) = entry.kind {
                    if let AgentLogKind::TurnStarted { turn_sequence, .. } = kind {
                        self.turn = turn_sequence;
                    }
                    if matches!(kind, AgentLogKind::AgentMessage { .. }) {
                        self.committed_message_seq = entry.seq;
                    }
                    self.approvals.observe(&AgentLogEntry {
                        agent_id: self.id.clone(),
                        seq: entry.seq,
                        appended_at: entry.appended_at,
                        kind,
                    });
                }
            }
        }
        if let Some(turn) = self.run {
            self.turn = turn;
            if self.after >= turn {
                self.pending.push_back(AguiEvent {
                    kind: "RUN_STARTED",
                    thread_id: self.id.to_string(),
                    run_id: turn.to_string(),
                    body: serde_json::json!({}),
                    timestamp: None,
                    seq: None,
                    live: false,
                });
            }
        }
        self.fill().await
    }

    async fn fill(&mut self) -> Result<(), AgentReadError> {
        let page = self.page(self.through).await?;
        if self
            .header
            .as_ref()
            .is_some_and(|header| header.created_at != page.created_at)
        {
            return Err(AgentReadError::NotFound);
        }
        let header = AgentEventStreamHeader {
            id: page.id.clone(),
            created_at: page.created_at.clone(),
            runtime_generation: page.runtime_generation,
            active_turn: page.active_turn,
            head: page.head,
        };
        self.observed_header = Some(header.clone());
        self.header = Some(header);
        self.queue_authorized(page);
        if self.through.is_some_and(|through| self.after >= through) {
            self.ended = true;
        }
        Ok(())
    }

    async fn wait(&mut self) -> Result<(), AgentReadError> {
        let mut force = false;
        while self.discarded_fragments == 0
            && self
                .held
                .as_ref()
                .is_none_or(|fragment| fragment.after_seq > self.after)
        {
            let deadline = self
                .authorization
                .as_ref()
                .and_then(AgentEventStreamAuthorization::revalidation_deadline);
            tokio::select! {
                biased;
                _ = tokio::time::sleep_until(self.fallback_due) => {
                    self.fallback_due = tokio::time::Instant::now() + Duration::from_secs(1);
                    break;
                }
                _ = async {
                    if let Some(deadline) = deadline { tokio::time::sleep_until(deadline).await }
                    else { pending::<()>().await }
                } => break,
                value = async {
                    match &mut self.heads { Some(heads) => heads.recv().await, None => pending().await }
                } => { if value.is_err() { self.heads = None; continue; } force = true; break; }
                value = async {
                    if self.held.is_some() { return pending().await; }
                    match &mut self.watching { Some(watching) => watching.recv().await, None => pending().await }
                } => {
                    match value {
                        Ok(fragment) => self.held = Some(fragment),
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => {
                            self.discarded_fragments += count;
                        }
                        Err(_) => { self.watching = None; continue; }
                    }
                    break;
                }
            }
        }
        let current = self.observe(force).await?;
        if self.header.as_ref().is_some_and(|header| {
            header.created_at != current.created_at || current.head < header.head
        }) {
            return Err(AgentReadError::NotFound);
        }
        if self.header.as_ref() != Some(&current) || self.after < current.head {
            return self.fill().await;
        }
        if self.discarded_fragments > 0 {
            let frames = std::mem::take(&mut self.discarded_fragments);
            self.pending.push_back(AguiEvent {
                kind: "STATE_DELTA", thread_id: self.id.to_string(), run_id: self.turn.to_string(),
                body: serde_json::json!({"delta": [{"op": "replace", "path": "/conversationGap", "value": {"frames": frames}}]}),
                timestamp: None, seq: None, live: true,
            });
        }
        self.queue_fragments(
            &current.id,
            current.runtime_generation,
            current.active_turn,
            current.head,
        );
        Ok(())
    }

    async fn observe(&mut self, force: bool) -> Result<AgentEventStreamHeader, AgentReadError> {
        let now = tokio::time::Instant::now();
        if !force && now.duration_since(self.observed_at) < Duration::from_secs(1) {
            if let Some(header) = &self.observed_header {
                return Ok(header.clone());
            }
        }
        let current = self.header().await?;
        self.observed_at = now;
        self.observed_header = Some(current.clone());
        Ok(current)
    }

    async fn validate(&mut self) -> Result<(), AgentReadError> {
        if let Some(authorization) = &self.authorization {
            if authorization
                .revalidation_deadline()
                .is_some_and(|deadline| tokio::time::Instant::now() >= deadline)
            {
                return Err(AgentReadError::Forbidden);
            }
            if !authorization.is_personal_owner() {
                // The fresh release fence also observes the head, so a live
                // batch cannot outlive durable output committed while blocked.
                self.observe(true).await?;
            }
        }
        let current = self.observe(false).await?;
        if self
            .header
            .as_ref()
            .is_some_and(|header| header.created_at == current.created_at)
        {
            Ok(())
        } else {
            Err(AgentReadError::NotFound)
        }
    }
}

pub(super) fn response(open: Following) -> Response {
    let stream = stream::unfold(open, |mut open| async move {
        loop {
            if let Some(event) = open.pending.pop_front() {
                if let Err(error) = open.validate().await {
                    if authority_lost(&error) {
                        return None;
                    }
                    return Some(interrupted(open));
                }
                if event.live && open.header != open.observed_header {
                    // Time-based observation also detects batches held under
                    // backpressure. Discard once, then catch up before the gap.
                    open.discarded_fragments += 1 + open.pending.len() as u64;
                    open.pending.clear();
                    continue;
                }
                return Some((Ok::<_, std::io::Error>(event.sse()), open));
            }
            if open.ended {
                return None;
            }
            let poll = Poll::new(Kind::Agui);
            let result = if open
                .header
                .as_ref()
                .is_some_and(|header| open.after < header.head)
            {
                open.fill().await
            } else {
                open.wait().await
            };
            if let Err(error) = result {
                if authority_lost(&error) {
                    return None;
                }
                return Some(interrupted(open));
            }
            poll.complete(!open.pending.is_empty());
        }
    });
    let mut response = Sse::new(stream_telemetry::observe(stream, Kind::Agui))
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response();
    response
        .headers_mut()
        .insert("cache-control", "no-cache, no-store".parse().unwrap());
    response
        .headers_mut()
        .insert("x-accel-buffering", "no".parse().unwrap());
    response
}

fn authority_lost(error: &AgentReadError) -> bool {
    use layer2_core::resource_access::{
        MembershipError, ResourceAccessError, ResourceAccessStoreError,
    };
    matches!(
        error,
        AgentReadError::Forbidden
            | AgentReadError::NotFound
            | AgentReadError::Access(ResourceAccessError::Access(_))
            | AgentReadError::Access(ResourceAccessError::Store(
                ResourceAccessStoreError::NotFound | ResourceAccessStoreError::Conflict
            ))
            | AgentReadError::Access(ResourceAccessError::Membership(
                MembershipError::NotMember | MembershipError::InvalidEvidence
            ))
    )
}

fn interrupted(
    mut open: Following,
) -> (
    Result<axum::response::sse::Event, std::io::Error>,
    Following,
) {
    open.pending.clear();
    open.ended = true;
    // A failed read is a broken transport, never an Agent outcome. Error the
    // body so finite readers cannot accept a partial successful replay.
    (Err(std::io::Error::other("history read interrupted")), open)
}
