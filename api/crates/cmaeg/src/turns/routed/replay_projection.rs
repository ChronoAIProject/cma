//! Persisted live-output projection. A pending effect is replayed exactly once.
use super::{storage, ExecutionError, FrozenTurn, Lease, Phase, ReplayEffect};
use crate::{
    providers::{OutputItem, RecoveryEvent, ResponsesEvent},
    telemetry::turn_trace::AssociationSlot,
    turns::{outcomes::TurnOutcome, projection::Projection},
};
use cmaeg_core::message::OutboundMessage;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReplayProjection {
    #[serde(default, skip_serializing_if = "AssociationSlot::is_empty")]
    pub trace: AssociationSlot,
    pub response_id: String,
    pub native_agent_id: Option<String>,
    pub last: Option<ProjectedFrame>,
    projection: Projection,
    thread_reply: bool,
    answer_attempted: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    primary_committed: bool,
    tool_replied: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProjectedFrame {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub trace_contract: bool,
    pub sequence: u64,
    pub frame_hash: String,
    pub terminal: bool,
    pub effect: ReplayEffect,
}
impl ReplayProjection {
    pub fn new(response_id: String) -> Self {
        Self {
            trace: AssociationSlot::default(),
            response_id,
            native_agent_id: None,
            last: None,
            projection: Projection::default(),
            thread_reply: false,
            answer_attempted: false,
            primary_committed: false,
            tool_replied: false,
        }
    }
    pub fn validate(&self, lease: &Lease) -> Result<(), ExecutionError> {
        let Phase::Accepted {
            response_id,
            durable_cursor,
        } = &lease.state.phase
        else {
            return Err(ExecutionError::Invalid);
        };
        if !self.projection.validate()
            || &self.response_id != response_id
            || self
                .native_agent_id
                .as_ref()
                .is_some_and(|id| id.is_empty() || id.len() > 256)
        {
            return Err(ExecutionError::Corrupt);
        }
        if let Some(last) = &self.last {
            if let Some(checkpoint) = &lease.state.delivery {
                if checkpoint.cursor == last.sequence
                    && (checkpoint.response_id != self.response_id
                        || checkpoint.frame_hash != last.frame_hash
                        || checkpoint.effect_hash != storage::hash(&storage::bytes(&last.effect)?)
                        || checkpoint.terminal != last.terminal)
                {
                    return Err(ExecutionError::Corrupt);
                }
            }
            if Some(last.sequence) == *durable_cursor
                && !lease.state.delivery.as_ref().is_some_and(|checkpoint| {
                    checkpoint.cursor == last.sequence && checkpoint.status.settled()
                })
            {
                return Err(ExecutionError::Corrupt);
            }
        }
        match (&self.last, durable_cursor) {
            (None, None) => Ok(()),
            (Some(last), cursor)
                if Some(last.sequence) == *cursor
                    || cursor.map_or(Some(0), |v| v.checked_add(1)) == Some(last.sequence) =>
            {
                if last.frame_hash.len() != 64
                    || !last.frame_hash.bytes().all(|v| v.is_ascii_hexdigit())
                {
                    return Err(ExecutionError::Corrupt);
                }
                Ok(())
            }
            _ => Err(ExecutionError::Corrupt),
        }
    }
    pub fn terminal_acknowledged(&self, lease: &Lease) -> bool {
        matches!((&self.last, &lease.state.phase),
            (Some(last), Phase::Accepted { durable_cursor: Some(cursor), .. })
            if last.terminal && last.sequence == *cursor)
    }

    pub fn retain_trace_contract(&self, frame: &mut RecoveryEvent) {
        // A writer rollback cannot change the execution digest contract of an
        // already admitted response, including frames not seen before restart.
        frame.trace_contract |=
            !self.trace.is_empty() || self.last.as_ref().is_some_and(|last| last.trace_contract);
    }

    pub fn pending_effect(
        &self,
        frame: &RecoveryEvent,
    ) -> Result<Option<ReplayEffect>, ExecutionError> {
        if frame.response_id != self.response_id {
            return Err(ExecutionError::Corrupt);
        }
        if let Some(last) = &self.last {
            if last.sequence == frame.sequence_number {
                if last.frame_hash
                    != super::replay_frame::hash(
                        frame,
                        last.trace_contract || frame.trace_contract,
                    )?
                {
                    return Err(ExecutionError::Corrupt);
                }
                return Ok(Some(last.effect.clone()));
            }
            if last.terminal || last.sequence.checked_add(1) != Some(frame.sequence_number) {
                return Err(ExecutionError::Corrupt);
            }
        } else if frame.sequence_number != 0 {
            return Err(ExecutionError::Corrupt);
        }
        Ok(None)
    }
    pub fn needs_tool_receipt(&self, frame: &RecoveryEvent, body: &FrozenTurn) -> bool {
        match &frame.event {
            ResponsesEvent::OutputTextDelta { .. }
                if super::custom_delivery::is_custom_origin(body) =>
            {
                body.delivery.auto_final && !self.tool_replied
            }
            ResponsesEvent::Completed { .. }
            | ResponsesEvent::Failed { .. }
            | ResponsesEvent::Incomplete { .. } => true,
            ResponsesEvent::OutputTextDone { .. }
            | ResponsesEvent::OutputItemDone {
                item: OutputItem::Message { .. },
                ..
            } => body.delivery.auto_final && !self.tool_replied,
            _ => false,
        }
    }
    /// Call only for a new contiguous frame; persist before delivering/acking it.
    pub fn project(
        &mut self,
        frame: &RecoveryEvent,
        body: &FrozenTurn,
        tool_replied: bool,
        failure_text: &str,
    ) -> Result<ReplayEffect, ExecutionError> {
        if self.pending_effect(frame)?.is_some() {
            return Err(ExecutionError::Invalid);
        }
        if frame.trace_contract {
            if let Some(association) = frame.trace {
                self.trace.bind(association);
            }
        }
        self.tool_replied |= tool_replied;
        if let ResponsesEvent::Created { thread_reply, .. } = &frame.event {
            self.thread_reply = *thread_reply;
        }
        let outcome = self
            .projection
            .event(frame.event.clone())
            .map_err(|_| ExecutionError::Invalid)?;
        let custom = super::custom_delivery::is_custom_origin(body);
        let custom_effect = outcome.as_ref().and_then(|outcome| {
            crate::delivery::local_stream::CustomStreamEffect::project(
                outcome,
                body.delivery.auto_final && !self.tool_replied,
            )
        });
        let primary = frame.raw["item"]["cmaeg_delivery"].as_str() == Some("primary_v1")
            && matches!(outcome, Some(TurnOutcome::MessageCommitted { .. }));
        self.primary_committed |= primary;
        let text = match outcome {
            Some(TurnOutcome::MessageCommitted { text })
                if body.delivery.auto_final && (!self.tool_replied || primary) =>
            {
                self.answer_attempted = true;
                Some(text)
            }
            Some(TurnOutcome::Completed { final_text })
                if body.delivery.auto_final && !self.tool_replied && !self.answer_attempted =>
            {
                self.answer_attempted |= final_text.is_some();
                final_text
            }
            Some(TurnOutcome::Failed { .. } | TurnOutcome::Incomplete { .. })
                if !self.tool_replied && !self.primary_committed =>
            {
                Some(failure_text.to_owned())
            }
            _ => None,
        };
        let effect = if custom {
            custom_effect.map_or(ReplayEffect::NoEffect, ReplayEffect::Custom)
        } else {
            text.map_or(ReplayEffect::NoEffect, |text| {
                ReplayEffect::Reply(Box::new(OutboundMessage {
                    text: Some(text),
                    blocks: vec![],
                    reply_to: None,
                    structured_content: None,
                    reply_with_received_sticker: false,
                    clear_native_attachments: false,
                    native_media_update: None,
                    native_attachments: vec![],
                    whatsapp_template: None,
                    thread_reply: self.thread_reply,
                }))
            })
        };
        self.last = Some(ProjectedFrame {
            trace_contract: frame.trace_contract,
            sequence: frame.sequence_number,
            frame_hash: super::replay_frame::hash(frame, frame.trace_contract)?,
            terminal: matches!(
                frame.event,
                ResponsesEvent::Completed { .. }
                    | ResponsesEvent::Failed { .. }
                    | ResponsesEvent::Incomplete { .. }
            ),
            effect: effect.clone(),
        });
        Ok(effect)
    }
}
