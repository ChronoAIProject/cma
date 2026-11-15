//! Join an outbound admission attempt to NyxID's server-issued ingress ID.
use super::{
    record::{Phase, Record},
    Context, Operation, Stage, Writer,
};
use std::time::SystemTime;
use uuid::Uuid;

#[derive(Debug)]
pub(super) struct SourceRequest {
    writer: Writer,
    operation: Option<Operation>,
    context: Context,
    linked: Option<Uuid>,
}

impl SourceRequest {
    pub(super) fn start(writer: Writer) -> Self {
        // Before NyxID admits a turn, only this local attempt has an identity.
        // Never substitute the event, response or conversation ID for a turn.
        let operation = Operation::start(writer.clone(), Stage::Ingress, None, None);
        Self {
            context: operation.context(),
            operation: Some(operation),
            writer,
            linked: None,
        }
    }

    pub(super) fn response(&mut self, request: Option<Uuid>, status: u16) {
        self.context.ingress_request_id = request;
        if let Some(operation) = self.operation.take() {
            operation.source_response(request, status);
        }
    }

    pub(super) fn bind(&mut self, turn: Uuid, remote_request: Option<Uuid>) {
        if self.linked.is_some() {
            return;
        }
        self.linked = Some(turn);
        // The schema-1 report joins pre-admission spans by request ID. Keep a
        // canonical link for both this local attempt and NyxID's ingress; the
        // pre-admission start/end records themselves remain turnless.
        self.link(turn, self.context.request_id);
        if let Some(request) = self.context.ingress_request_id.or(remote_request) {
            self.link(turn, request);
        }
    }

    fn link(&self, turn: Uuid, ingress_request_id: Uuid) {
        self.writer.send(Record {
            timestamp: SystemTime::now(),
            context: Context {
                turn_id: Some(turn),
                ingress_request_id: Some(ingress_request_id),
                ..self.context
            },
            stage: Stage::Gateway,
            phase: Phase::Link,
            outcome: None,
            duration_ms: None,
            http_status: None,
            part_index: None,
            related_turn_id: Some(turn),
        });
    }
}

#[cfg(test)]
mod tests;
