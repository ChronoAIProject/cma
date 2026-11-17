//! Reader for associations returned by an authenticated provider response.
use super::{
    parent::{single, uuid_v4},
    Association, AssociationSlot, BindResult,
};
use axum::http::HeaderMap;
use serde_json::Value;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Observation {
    Missing,
    Bound,
    Duplicate,
    Conflict,
    Invalid,
}

/// Identity comes from the existing authorized provider/recovery operation.
/// Nothing here can return an execution error or modify its request body/key.
#[derive(Debug)]
pub(crate) struct ProviderReader {
    response_id: Option<String>,
    association: AssociationSlot,
    header_turn: Option<Uuid>,
    request_id: Option<Uuid>,
    pub(super) source: Option<super::source::SourceRequest>,
}

impl ProviderReader {
    /// Check both gates before allocating or inspecting diagnostic input.
    pub(crate) fn new(
        enabled: bool,
        negotiated: bool,
        response_id: Option<&str>,
    ) -> Option<Box<Self>> {
        if !enabled || !negotiated {
            return None;
        }
        if response_id.is_some_and(|id| !valid_response_id(id)) {
            return None;
        }
        // Keep default-path request and stream futures pointer-sized at this
        // boundary. The larger diagnostic state is allocated only after both gates.
        Some(Box::new(Self {
            response_id: response_id.map(str::to_owned),
            association: AssociationSlot::default(),
            header_turn: None,
            request_id: None,
            source: None,
        }))
    }

    /// Only admission requests need a pre-admission source span. Recovery
    /// readers keep their existing worker spans and add no per-poll records.
    pub(crate) fn begin_source(&mut self) {
        self.source = super::runtime::writer().map(super::source::SourceRequest::start);
    }

    pub(crate) fn response(&mut self, headers: &HeaderMap, status: u16) {
        self.headers(headers);
        let turn = self.turn_id();
        if let Some(source) = &mut self.source {
            source.response(self.request_id, status);
            if let Some(turn) = turn {
                source.bind(turn, self.request_id);
            }
        }
    }

    pub(crate) fn headers(&mut self, headers: &HeaderMap) -> Observation {
        let request = single(headers, "x-request-id").and_then(uuid_v4);
        let turn = single(headers, "x-nyxid-trace-id").and_then(uuid_v4);
        // Ingress request metadata is useful even if admission has not bound.
        self.request_id = self.request_id.or(request);
        match (request, turn) {
            (Some(request), Some(turn)) if request != turn => {
                if self.turn_id().is_some_and(|existing| existing != turn) {
                    return Observation::Conflict;
                }
                if self.header_turn == Some(turn) {
                    return Observation::Duplicate;
                }
                self.header_turn = Some(turn);
                Observation::Bound
            }
            _ => Observation::Missing,
        }
    }

    pub(crate) fn turn_id(&self) -> Option<Uuid> {
        self.association
            .get()
            .map(|value| value.turn_id)
            .or(self.header_turn)
    }

    pub(crate) fn association(&self) -> Option<Association> {
        self.association.get().or_else(|| {
            Some(Association {
                turn_id: self.header_turn?,
                parent_span_id: None,
                ingress_request_id: self.request_id?,
            })
        })
    }

    pub(crate) fn snapshot(&mut self, response: &Value) -> Observation {
        let Some(response_id) = response.get("id").and_then(Value::as_str) else {
            return Observation::Invalid;
        };
        if !valid_response_id(response_id) {
            return Observation::Invalid;
        }
        if self
            .response_id
            .as_deref()
            .is_some_and(|expected| expected != response_id)
        {
            return Observation::Conflict;
        }
        if self.response_id.is_none() {
            self.response_id = Some(response_id.to_owned());
        }
        self.bind(response.get("nyxid_trace"))
    }

    pub(crate) fn event(&mut self, event: &Value) -> Observation {
        match event.get("type").and_then(Value::as_str) {
            Some("response.nyxid_trace") => {
                // A metadata event cannot invent or replace the response it is
                // associated with. Created/snapshot/recovery owns that identity.
                if event.get("response_id").and_then(Value::as_str) != self.response_id.as_deref()
                    || self.response_id.is_none()
                {
                    return Observation::Conflict;
                }
                self.bind(event.get("nyxid_trace"))
            }
            Some(
                "response.created"
                | "response.in_progress"
                | "response.completed"
                | "response.failed"
                | "response.incomplete",
            ) => event
                .get("response")
                .map_or(Observation::Missing, |response| self.snapshot(response)),
            _ => Observation::Missing,
        }
    }

    fn bind(&mut self, value: Option<&Value>) -> Observation {
        let Some(value) = value else {
            return Observation::Missing;
        };
        let Some(association) = Association::parse(value) else {
            return Observation::Invalid;
        };
        if self
            .turn_id()
            .is_some_and(|turn| turn != association.turn_id)
        {
            return Observation::Conflict;
        }
        let result = match self.association.bind(association) {
            BindResult::Bound => Observation::Bound,
            BindResult::Duplicate => Observation::Duplicate,
            BindResult::Conflict => Observation::Conflict,
        };
        if result == Observation::Bound {
            if let Some(source) = &mut self.source {
                source.bind(association.turn_id, Some(association.ingress_request_id));
            }
        }
        result
    }
}

fn valid_response_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests;
