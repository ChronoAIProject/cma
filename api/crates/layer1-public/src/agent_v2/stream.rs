use super::{
    cursor, error::Error, parse, projection, shapes::DurableEvent, snapshot, AgentV2State,
};
use crate::stream_telemetry::{self, Kind, Poll};
use crate::AuthenticatedResourceCaller;
use axum::{
    extract::{Path, RawQuery, State},
    http::HeaderMap,
    response::{
        sse::{Event, KeepAlive},
        IntoResponse, Response, Sse,
    },
};
use layer2_core::{
    sandbox::{
        agent_io::{AgentEventKind, AgentMessageRole},
        AgentEventStreamAuthorization, AgentEventStreamHeader, AgentHeadSubscription,
        AgentLiveDelivery, AgentLiveSubscription,
    },
    AgentId,
};
use std::{collections::VecDeque, convert::Infallible, time::Duration};
#[cfg(test)]
#[path = "telemetry_tests.rs"]
mod telemetry_tests;
#[cfg(test)]
#[path = "stream_tests.rs"]
mod tests;

#[utoipa::path(get, path = "/api/v2/agents/{agent_id}/events", tag = "agent events v2",
    params(("agent_id" = String, Path), ("after_cursor" = Option<String>, Query),
    ("follow" = Option<bool>, Query), ("Last-Event-ID" = Option<String>, Header), ("Prefer" = Option<String>, Header)),
    responses((status = 200, body = String, content_type = "text/event-stream"),
    (status = 400, body = crate::rest::ProblemDocument), (status = 404, body = crate::rest::ProblemDocument),
    (status = 409, body = crate::rest::ProblemDocument), (status = 503, body = crate::rest::ProblemDocument)))]
pub(super) async fn read(
    State(state): State<AgentV2State>,
    auth: AuthenticatedResourceCaller,
    Path(id): Path<String>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
) -> Response {
    match open(state, auth, AgentId(id), raw, headers).await {
        Ok(open) => response(open),
        Err(error) => error.response(),
    }
}

async fn open(
    mut state: AgentV2State,
    auth: AuthenticatedResourceCaller,
    id: AgentId,
    raw: Option<String>,
    headers: HeaderMap,
) -> Result<Following, Error> {
    let (after, follow) = parse::tail(raw, &headers)?;
    let message_bindings = projection::message_bindings(&headers);
    let mut authorization = snapshot::authorize(&state, &auth, &id).await?;
    let external = super::external_approval::Preference::new(
        projection::preference(&headers, super::external_approval::PREFERENCE),
        &state.external_review_origin,
        &authorization,
    );
    let observation = state.reads.observe_event_stream(&mut authorization)?;
    state.event_wake |= observation.is_some();
    // Subscription precedes the initial head observation, including its IO.
    let watching = if follow {
        match &state.updates {
            Some(updates) => updates.admit_live(&id).await.map_err(Error::Live)?,
            None => None,
        }
    } else {
        None
    };
    let heads = if follow && state.event_wake {
        state
            .updates
            .as_ref()
            .and_then(|updates| updates.subscribe_head(&id))
    } else {
        None
    };
    let fallback_due = tokio::time::Instant::now() + state.fallback_poll;
    let header = state.reads.event_stream_header(&authorization).await?;
    let identity = cursor::incarnation(&header);
    let after = after
        .as_deref()
        .map(|value| cursor::parse_event(value, &identity, header.head))
        .transpose()?
        .unwrap_or(0);
    let mut open = Following {
        state,
        authorization,
        observation,
        through: (!follow).then_some(header.head),
        header,
        identity,
        after,
        follow,
        message_bindings,
        live_timestamps: projection::live_timestamps(&headers),
        tool_output: projection::tool_output(&headers),
        retained_output: projection::retained_output(&headers),
        external,
        pending: VecDeque::new(),
        deferred_live: VecDeque::new(),
        watching,
        heads,
        fallback_due,
        needs_catchup: false,
        release_failed: false,
        held: None,
        committed_message: after,
        initial_gap: follow,
        discarded_fragments: 0,
        live_batch_needs_validation: false,
        ended: false,
    };
    open.fill().await?;
    Ok(open)
}

#[derive(Clone)]
pub(super) struct Frame {
    pub event: Event,
    pub live: bool,
    pub fragment: Option<AgentLiveDelivery>,
}
pub(super) struct Following {
    pub state: AgentV2State,
    pub(super) authorization: AgentEventStreamAuthorization,
    pub(super) observation:
        Option<layer2_core::sandbox::agent_observation::AgentObservationSubscription>,
    pub header: AgentEventStreamHeader,
    pub identity: String,
    pub after: u64,
    through: Option<u64>,
    pub follow: bool,
    message_bindings: bool,
    pub live_timestamps: bool,
    tool_output: bool,
    retained_output: bool,
    external: super::external_approval::Preference,
    pub pending: VecDeque<Frame>,
    pub(super) deferred_live: VecDeque<Frame>,
    pub watching: Option<AgentLiveSubscription>,
    pub held: Option<AgentLiveDelivery>,
    pub committed_message: u64,
    pub(super) initial_gap: bool,
    pub(super) discarded_fragments: u64,
    pub(super) live_batch_needs_validation: bool,
    ended: bool,
    pub(super) heads: Option<AgentHeadSubscription>,
    pub(super) fallback_due: tokio::time::Instant,
    pub(super) needs_catchup: bool,
    release_failed: bool,
}

impl Following {
    pub(super) async fn fill(&mut self) -> Result<(), Error> {
        let window = projection::window(
            &self.state,
            &self.authorization,
            self.after,
            self.through,
            200,
            self.message_bindings,
        )
        .await?;
        if cursor::incarnation(&window.header) != self.identity {
            return Err(Error::Incarnation);
        }
        let mut items = projection::items(
            &window,
            &self.identity,
            self.tool_output,
            self.retained_output,
        )?;
        self.external.project(&mut items)?;
        self.header = window.header;
        for item in items {
            if matches!(
                item.kind,
                Some(AgentEventKind::Message {
                    role: AgentMessageRole::Assistant,
                    ..
                })
            ) {
                self.committed_message = item.sequence;
            }
            let id = item.cursor.clone();
            let sequence = item.sequence;
            let durable = DurableEvent {
                agent_id: self.header.id.to_string(),
                incarnation: self.identity.clone(),
                item,
            };
            let body = serde_json::to_string(&durable).map_err(|_| Error::History)?;
            if body.len() > projection::MAX_BYTES {
                return Err(Error::History);
            }
            self.pending.push_back(Frame {
                event: Event::default().event("cma.agent.event").id(id).data(body),
                live: false,
                fragment: None,
            });
            self.after = sequence;
        }
        if self.after >= window.through && !self.follow {
            self.ended = true;
        }
        self.needs_catchup = false;
        self.queue_tail();
        Ok(())
    }

    pub(super) fn queue_tail(&mut self) {
        // Complete durable catch-up always precedes live fragments or gaps.
        if self.pending.is_empty() && self.follow && self.after == self.header.head {
            if self.initial_gap {
                self.initial_gap = false;
                self.queue_gap(None, "reconnected");
            }
            self.revalidate_deferred_live();
            if self.discarded_fragments > 0 {
                let frames = std::mem::take(&mut self.discarded_fragments);
                self.queue_gap(Some(frames), "lagged");
            }
            if self.deferred_live.is_empty() {
                self.queue_live();
            } else {
                self.pending.append(&mut self.deferred_live);
                self.live_batch_needs_validation =
                    self.pending.iter().any(|frame| frame.fragment.is_some());
            }
        }
    }

    async fn release(&mut self, frame: &Frame) -> bool {
        if self.state.event_wake
            && self
                .authorization
                .revalidation_deadline()
                .is_some_and(|deadline| deadline <= tokio::time::Instant::now())
        {
            self.release_failed = true;
            return false;
        }
        if frame.fragment.is_some() {
            if self
                .state
                .reads
                .validate_event_stream_standing(&self.authorization)
                .await
                .is_err()
            {
                self.release_failed = true;
                return false;
            }
            if !self.live_batch_needs_validation && self.observation.is_none() {
                return true;
            }
            self.live_batch_needs_validation = false;
            let current = if self.state.event_wake {
                self.state
                    .reads
                    .current_event_stream_header(&self.authorization)
                    .await
            } else {
                self.state
                    .reads
                    .event_stream_header(&self.authorization)
                    .await
            };
            let Ok(current) = current else {
                self.release_failed = true;
                return false;
            };
            if current.runtime_generation != self.header.runtime_generation
                || current.active_turn != self.header.active_turn
                || current.head != self.header.head
            {
                // Durable events can race a live batch. Preserve the buffered
                // source fragments, catch up durably, then recheck their validity.
                let pending = std::mem::take(&mut self.pending);
                self.header = current;
                self.needs_catchup = self.state.event_wake;
                self.deferred_live.push_back(frame.clone());
                self.deferred_live.extend(pending);
                return false;
            }
            true
        } else {
            let allowed = self
                .state
                .reads
                .validate_event_stream(&self.authorization)
                .await
                .is_ok();
            self.release_failed = !allowed;
            allowed
        }
    }

    fn revalidate_deferred_live(&mut self) {
        let deferred = std::mem::take(&mut self.deferred_live);
        for frame in deferred {
            if let Some(fragment) = &frame.fragment {
                if fragment.agent_id == self.header.id
                    && Some(fragment.turn_sequence) == self.header.active_turn
                    && Some(fragment.runtime_generation) != self.header.runtime_generation
                {
                    self.discarded_fragments += 1;
                }
                if !self.current_live_fragment(fragment) || fragment.after_seq > self.header.head {
                    continue;
                }
            }
            self.deferred_live.push_back(frame);
        }
    }
}

fn response(open: Following) -> Response {
    let external = open.external.clone();
    let stream = futures_util::stream::unfold(open, |mut open| async move {
        loop {
            if let Some(frame) = open.pending.pop_front() {
                if !open.release(&frame).await {
                    if !frame.live
                        || (open.state.event_wake && open.release_failed)
                        || (!open.state.event_wake
                            && open
                                .state
                                .reads
                                .validate_event_stream(&open.authorization)
                                .await
                                .is_err())
                    {
                        return None;
                    }
                    continue;
                }
                if let Some(fragment) = &frame.fragment {
                    stream_telemetry::agent_live_delivered(fragment.published_at());
                }
                return Some((Ok::<_, Infallible>(frame.event), open));
            }
            if open.ended {
                return None;
            }
            let result = if open.state.event_wake
                && open.follow
                && open.after == open.header.head
                && !open.needs_catchup
            {
                // Keep the nested replay future off the response poll stack.
                Box::pin(open.wait_for_change()).await
            } else {
                if open.after == open.header.head && !open.needs_catchup {
                    let wake = tokio::time::Instant::now() + Duration::from_millis(100);
                    let wake = open
                        .authorization
                        .revalidation_deadline()
                        .map_or(wake, |deadline| deadline.min(wake));
                    tokio::time::sleep_until(wake).await;
                }
                let poll = Poll::new(Kind::AgentEventsV2);
                let result = open.fill().await;
                if result.is_ok() {
                    poll.complete(!open.pending.is_empty());
                }
                result
            };
            if let Err(error) = result {
                if open
                    .state
                    .reads
                    .validate_event_stream(&open.authorization)
                    .await
                    .is_err()
                {
                    return None;
                }
                open.pending.clear();
                open.ended = true;
                let body = serde_json::json!({"code": error.code(), "message": error.message()});
                return Some((
                    Ok(Event::default()
                        .event("cma.agent.error")
                        .data(body.to_string())),
                    open,
                ));
            }
        }
    });
    let mut response = Sse::new(stream_telemetry::observe(stream, Kind::AgentEventsV2))
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response();
    response
        .headers_mut()
        .insert("cache-control", "private, no-store".parse().unwrap());
    response
        .headers_mut()
        .insert("x-accel-buffering", "no".parse().unwrap());
    external.headers(response.headers_mut());
    response
}
