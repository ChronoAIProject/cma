use super::{
    cursor, error::Error, parse, projection, shapes::DurableEvent, snapshot, AgentV2State,
};
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
        AgentEventStreamAuthorization, AgentEventStreamHeader, AgentLiveFragment,
        AgentLiveSubscription,
    },
    AgentId,
};
use std::{collections::VecDeque, convert::Infallible, time::Duration};
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
    state: AgentV2State,
    auth: AuthenticatedResourceCaller,
    id: AgentId,
    raw: Option<String>,
    headers: HeaderMap,
) -> Result<Following, Error> {
    let (after, follow) = parse::tail(raw, &headers)?;
    let message_bindings = projection::message_bindings(&headers);
    let authorization = snapshot::authorize(&state, &auth, &id).await?;
    // Subscription precedes the initial head observation, including its IO.
    let watching = if follow {
        state
            .updates
            .as_ref()
            .and_then(|updates| updates.subscribe_live(&id))
    } else {
        None
    };
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
        through: (!follow).then_some(header.head),
        header,
        identity,
        after,
        follow,
        message_bindings,
        pending: VecDeque::new(),
        watching,
        held: None,
        committed_message: after,
        initial_gap: follow,
        discarded_fragments: 0,
        ended: false,
    };
    open.fill().await?;
    Ok(open)
}

pub(super) struct Frame {
    pub event: Event,
    pub live: bool,
    pub fragment: bool,
}
pub(super) struct Following {
    pub state: AgentV2State,
    authorization: AgentEventStreamAuthorization,
    pub header: AgentEventStreamHeader,
    pub identity: String,
    pub after: u64,
    through: Option<u64>,
    pub follow: bool,
    message_bindings: bool,
    pub pending: VecDeque<Frame>,
    pub watching: Option<AgentLiveSubscription>,
    pub held: Option<AgentLiveFragment>,
    pub committed_message: u64,
    initial_gap: bool,
    discarded_fragments: u64,
    ended: bool,
}

impl Following {
    async fn fill(&mut self) -> Result<(), Error> {
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
        let items = projection::items(&window, &self.identity)?;
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
                fragment: false,
            });
            self.after = sequence;
        }
        if self.after >= window.through && !self.follow {
            self.ended = true;
        }
        // Complete durable catch-up always precedes live fragments or gaps.
        if self.pending.is_empty() && self.follow && self.after == self.header.head {
            if self.initial_gap {
                self.initial_gap = false;
                self.queue_gap(None, "reconnected");
            }
            if self.discarded_fragments > 0 {
                let frames = std::mem::take(&mut self.discarded_fragments);
                self.queue_gap(Some(frames), "lagged");
            }
            self.queue_live();
        }
        Ok(())
    }

    async fn release(&mut self, frame: &Frame) -> bool {
        if frame.fragment {
            let Ok(current) = self
                .state
                .reads
                .event_stream_header(&self.authorization)
                .await
            else {
                return false;
            };
            if current.head != self.header.head
                || current.runtime_generation != self.header.runtime_generation
                || current.active_turn != self.header.active_turn
            {
                // The buffered live batch is now stale. Refill durable history
                // before deciding whether any newly observed fragment is safe.
                self.discarded_fragments +=
                    1 + self.pending.iter().filter(|frame| frame.fragment).count() as u64;
                self.pending.clear();
                return false;
            }
            true
        } else {
            self.state
                .reads
                .validate_event_stream(&self.authorization)
                .await
                .is_ok()
        }
    }
}

fn response(open: Following) -> Response {
    let stream = futures_util::stream::unfold(open, |mut open| async move {
        loop {
            if let Some(frame) = open.pending.pop_front() {
                if !open.release(&frame).await {
                    if !frame.live
                        || open
                            .state
                            .reads
                            .validate_event_stream(&open.authorization)
                            .await
                            .is_err()
                    {
                        return None;
                    }
                    continue;
                }
                return Some((Ok::<_, Infallible>(frame.event), open));
            }
            if open.ended {
                return None;
            }
            if open.after == open.header.head {
                let wake = tokio::time::Instant::now() + Duration::from_millis(100);
                let wake = open
                    .authorization
                    .revalidation_deadline()
                    .map_or(wake, |deadline| deadline.min(wake));
                tokio::time::sleep_until(wake).await;
            }
            if let Err(error) = open.fill().await {
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
    let mut response = Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response();
    response
        .headers_mut()
        .insert("cache-control", "private, no-store".parse().unwrap());
    response
        .headers_mut()
        .insert("x-accel-buffering", "no".parse().unwrap());
    response
}
