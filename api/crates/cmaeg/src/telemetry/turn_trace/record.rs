//! The producer boundary accepts only fixed-size, typed metadata.
use super::SpanId;
use serde::Serialize;
use std::time::SystemTime;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use uuid::Uuid;

pub(super) const MAX_RECORD_BYTES: usize = 2048;

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Stage {
    Gateway,
    Channel,
    ChannelPart,
    Platform,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Outcome {
    Completed,
    Unknown,
    Silent,
    Confirmed,
    Rejected,
    Partial,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Phase {
    Start,
    End,
    Milestone,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Context {
    pub turn_id: Option<Uuid>,
    pub span_id: SpanId,
    pub parent_span_id: Option<SpanId>,
    pub request_id: Uuid,
    pub ingress_request_id: Option<Uuid>,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Record {
    pub timestamp: SystemTime,
    pub context: Context,
    pub stage: Stage,
    pub phase: Phase,
    pub outcome: Option<Outcome>,
    pub duration_ms: Option<u64>,
    pub http_status: Option<u16>,
    pub part_index: Option<u32>,
    pub related_turn_id: Option<Uuid>,
}

impl Record {
    /// Serialization and string formatting run only on the bounded worker.
    pub(super) fn encode(self, instance_id: Uuid) -> Option<Vec<u8>> {
        let context = self.context;
        let wire = Wire {
            target: "nyxid::turn_trace",
            schema_version: 1,
            service: "cma",
            instance_id,
            timestamp: OffsetDateTime::from(self.timestamp).format(&Rfc3339).ok()?,
            turn_id: context.turn_id,
            trace_id: context.turn_id.map(|id| id.simple().to_string()),
            span_id: context.span_id.to_string(),
            parent_span_id: context.parent_span_id.map(|span| span.to_string()),
            request_id: context.request_id,
            ingress_request_id: context.ingress_request_id,
            phase: self.phase,
            stage: self.stage,
            outcome: self.outcome,
            duration_ms: self.duration_ms,
            http_status: self
                .http_status
                .filter(|status| (100..=599).contains(status)),
            part_index: self.part_index,
            related_turn_id: self.related_turn_id,
        };
        let mut bytes = serde_json::to_vec(&wire).ok()?;
        bytes.push(b'\n');
        (bytes.len() <= MAX_RECORD_BYTES).then_some(bytes)
    }
}

#[derive(Serialize)]
struct Wire {
    target: &'static str,
    schema_version: u8,
    service: &'static str,
    instance_id: Uuid,
    timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    turn_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    trace_id: Option<String>,
    span_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_span_id: Option<String>,
    request_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    ingress_request_id: Option<Uuid>,
    phase: Phase,
    stage: Stage,
    #[serde(skip_serializing_if = "Option::is_none")]
    outcome: Option<Outcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    http_status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    part_index: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    related_turn_id: Option<Uuid>,
}
