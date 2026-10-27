use super::{
    AgentCursor, AgentFrame, LiveGapReason,
    cursor::{identifier, valid_incarnation},
    validation,
};
use crate::{
    Error, Result,
    stream::{EventStream, ServerEvent},
};
use std::future::Future;

/// Pull-based observation that acknowledges only after the complete consumer callback succeeds.
pub struct AgentEventStream {
    inner: EventStream,
    agent_id: String,
    incarnation: Option<String>,
    sequence: u64,
    acknowledged: Option<AgentCursor>,
    ended: bool,
}
impl AgentEventStream {
    pub(super) fn new(inner: EventStream, agent_id: &str, after: Option<&AgentCursor>) -> Self {
        Self {
            inner,
            agent_id: agent_id.into(),
            incarnation: after.map(|c| c.incarnation().into()),
            sequence: after.map_or(0, AgentCursor::sequence),
            acknowledged: after.cloned(),
            ended: false,
        }
    }
    /// Last complete durable frame whose consumer returned success, or the supplied resume cursor.
    pub fn acknowledged_cursor(&self) -> Option<&AgentCursor> {
        self.acknowledged.as_ref()
    }
    /// Apply one complete frame. Consumer failure closes observation without advancing its cursor.
    /// Persist normalized state and this frame's cursor together inside the callback before returning success.
    pub async fn consume_next<F, Fut>(&mut self, consumer: F) -> Option<Result<()>>
    where
        F: FnOnce(AgentFrame) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        if self.ended {
            return None;
        }
        let Some(frame) = self.inner.next().await else {
            self.ended = true;
            return None;
        };
        let result = async {
            let frame = self.decode(frame?)?;
            let cursor = match &frame {
                AgentFrame::Event(event) => Some(event.cursor.clone()),
                _ => None,
            };
            let failure = match &frame {
                AgentFrame::Error(error) => Some(format!(
                    "Agent observation ended: {}: {}",
                    error.code, error.message
                )),
                _ => None,
            };
            consumer(frame).await?;
            if let Some(cursor) = cursor {
                self.acknowledged = Some(cursor);
            }
            if let Some(message) = failure {
                return Err(Error::transport(message));
            }
            Ok(())
        }
        .await;
        if result.is_err() {
            self.ended = true;
        }
        Some(result)
    }
    fn decode(&mut self, frame: ServerEvent) -> Result<AgentFrame> {
        let parsed = match frame.event.as_str() {
            "cma.agent.event" => {
                AgentFrame::Event(serde_json::from_value(frame.data).map_err(invalid)?)
            }
            "cma.agent.live" => {
                AgentFrame::Live(serde_json::from_value(frame.data).map_err(invalid)?)
            }
            "cma.agent.live_gap" => {
                AgentFrame::LiveGap(serde_json::from_value(frame.data).map_err(invalid)?)
            }
            "cma.agent.error" => {
                AgentFrame::Error(serde_json::from_value(frame.data).map_err(invalid)?)
            }
            _ => return Err(Error::protocol("unsupported Agent stream event")),
        };
        if !matches!(parsed, AgentFrame::Event(_)) && frame.explicit_id.is_some() {
            return Err(Error::protocol(
                "ephemeral Agent frame must not carry an SSE ID",
            ));
        }
        match &parsed {
            AgentFrame::Event(event) => {
                self.identity(&event.agent_id, &event.incarnation)?;
                if frame.explicit_id.as_deref() != Some(event.cursor.as_str())
                    || event.cursor.incarnation() != event.incarnation
                    || event.cursor.sequence() != event.sequence
                    || self.sequence.checked_add(1) != Some(event.sequence)
                {
                    return Err(Error::protocol(
                        "Agent durable envelope or cursor is inconsistent",
                    ));
                }
                validation::payload(
                    event.sequence,
                    &event.appended_at,
                    event.kind.as_ref(),
                    event.response_binding.as_ref(),
                )?;
                self.sequence = event.sequence;
            }
            AgentFrame::Live(event) => {
                self.identity(&event.agent_id, &event.incarnation)?;
                if event
                    .observed_at
                    .as_ref()
                    .is_some_and(|at| identifier(at).is_err())
                {
                    return Err(Error::protocol("invalid live observation timestamp"));
                }
                if event.turn_sequence == 0
                    || event.turn_sequence > event.after_sequence
                    || event.after_sequence > self.sequence
                {
                    return Err(Error::protocol(
                        "Agent live fragment precedes its durable anchor",
                    ));
                }
            }
            AgentFrame::LiveGap(event) => {
                self.identity(&event.agent_id, &event.incarnation)?;
                if (event.reason == LiveGapReason::Reconnected && event.frames.is_some())
                    || event
                        .turn_sequence
                        .is_some_and(|turn| turn == 0 || turn > self.sequence)
                {
                    return Err(Error::protocol(
                        "Agent live gap claims an invalid turn or loss",
                    ));
                }
            }
            AgentFrame::Error(error) => {
                if identifier(&error.code).is_err() {
                    return Err(Error::protocol("invalid Agent stream error"));
                }
            }
        }
        Ok(parsed)
    }
    fn identity(&mut self, agent: &str, incarnation: &str) -> Result<()> {
        if agent != self.agent_id
            || !valid_incarnation(incarnation)
            || self
                .incarnation
                .as_ref()
                .is_some_and(|known| known != incarnation)
        {
            return Err(Error::protocol(
                "Agent stream changed resource identity or incarnation",
            ));
        }
        self.incarnation = Some(incarnation.into());
        Ok(())
    }
}
fn invalid(_: serde_json::Error) -> Error {
    Error::protocol("invalid Agent stream document")
}
