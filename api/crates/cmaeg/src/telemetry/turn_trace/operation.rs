use super::{
    record::{Context, Phase, Record},
    Outcome, SpanId, Stage, Writer,
};
use axum::http::{HeaderMap, HeaderValue};
use std::time::{Instant, SystemTime};
use uuid::Uuid;

/// One actual operation captures its IDs once. All descendants derive from
/// this captured context, even if a newer conversation turn starts meanwhile.
#[derive(Debug)]
pub(crate) struct Operation {
    writer: Writer,
    context: Context,
    stage: Stage,
    part_index: Option<u32>,
    started: Instant,
    finished: bool,
}

impl Operation {
    pub(crate) fn start(
        writer: Writer,
        stage: Stage,
        parent: Option<Context>,
        part_index: Option<u32>,
    ) -> Self {
        let request_id = Uuid::new_v4();
        let context = Context {
            turn_id: parent.and_then(|parent| parent.turn_id),
            span_id: SpanId::from_request(request_id),
            parent_span_id: parent.map(|parent| parent.span_id),
            request_id,
            ingress_request_id: parent.and_then(|parent| parent.ingress_request_id),
        };
        Self::with_context(writer, stage, context, part_index)
    }

    pub(super) fn bound(writer: Writer, stage: Stage, association: super::Association) -> Self {
        let request_id = Uuid::new_v4();
        Self::with_context(
            writer,
            stage,
            Context {
                turn_id: Some(association.turn_id),
                span_id: SpanId::from_request(request_id),
                parent_span_id: association.parent_span_id,
                request_id,
                ingress_request_id: Some(association.ingress_request_id),
            },
            None,
        )
    }

    fn with_context(
        writer: Writer,
        stage: Stage,
        context: Context,
        part_index: Option<u32>,
    ) -> Self {
        let operation = Self {
            writer,
            context,
            stage,
            part_index,
            started: Instant::now(),
            finished: false,
        };
        operation.emit(Phase::Start, None, None, None);
        operation
    }

    pub(crate) fn context(&self) -> Context {
        self.context
    }

    pub(super) fn link(&self) {
        self.emit(Phase::Link, None, None, None);
    }

    /// Only tracing-enabled callers construct an operation. Headers remain
    /// outside request bodies and all execution identity/signature inputs.
    pub(crate) fn headers(&self, headers: &mut HeaderMap) {
        headers.insert(
            "x-request-id",
            HeaderValue::from_str(&self.context.request_id.to_string())
                .expect("UUID is a valid header"),
        );
        if let Some(turn_id) = self.context.turn_id {
            let parent = format!("00-{}-{}-01", turn_id.simple(), self.context.span_id);
            headers.insert(
                "traceparent",
                HeaderValue::from_str(&parent).expect("typed W3C context is a valid header"),
            );
        }
    }

    pub(crate) fn finish(mut self, outcome: Outcome, http_status: Option<u16>) {
        self.end(outcome, http_status);
    }

    pub(super) fn source_response(mut self, ingress_request_id: Option<Uuid>, status: u16) {
        self.context.ingress_request_id = ingress_request_id;
        self.end(Outcome::Completed, Some(status));
    }

    fn end(&mut self, outcome: Outcome, http_status: Option<u16>) {
        if self.finished {
            return;
        }
        self.finished = true;
        let duration = self.started.elapsed().as_millis().min(u64::MAX as u128) as u64;
        self.emit(Phase::End, Some(outcome), Some(duration), http_status);
    }

    fn emit(
        &self,
        phase: Phase,
        outcome: Option<Outcome>,
        duration_ms: Option<u64>,
        http_status: Option<u16>,
    ) {
        self.writer.send(Record {
            timestamp: SystemTime::now(),
            context: self.context,
            stage: self.stage,
            phase,
            outcome,
            duration_ms,
            http_status,
            part_index: self.part_index,
            related_turn_id: None,
        });
    }
}

impl Drop for Operation {
    fn drop(&mut self) {
        // Cancellation or lost transport completion cannot prove rejection or
        // absence of platform delivery. A trace never triggers another send.
        self.end(Outcome::Unknown, None);
    }
}
