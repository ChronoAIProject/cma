use super::{parent::uuid_v4, SpanId};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Only a negotiated provider response can establish this association. This
/// fixed-size representation is never used in an execution key or comparison.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Association {
    pub turn_id: Uuid,
    // Response UUID headers identify a turn but carry no NyxID parent span.
    pub parent_span_id: Option<SpanId>,
    pub ingress_request_id: Uuid,
}

#[derive(Serialize)]
struct Wire {
    version: u8,
    turn_id: String,
    trace_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_span_id: Option<String>,
    ingress_request_id: String,
}

impl Association {
    pub(crate) fn parse(value: &Value) -> Option<Self> {
        // The negotiated wire annotation always requires the canonical parent.
        // Only our retained header correlation is allowed to omit it.
        value.get("parent_span_id")?.as_str()?;
        Self::retained(value)
    }

    fn retained(value: &Value) -> Option<Self> {
        // Validate the closed borrowed shape without copying provider strings.
        // UUID and span parsers enforce their canonical grammar and bounds.
        let object = value.as_object()?;
        let has_parent = object.contains_key("parent_span_id");
        if object.len() != if has_parent { 5 } else { 4 } || object.get("version")?.as_u64()? != 1 {
            return None;
        }
        let turn_id = uuid_v4(object.get("turn_id")?.as_str()?)?;
        if object.get("trace_id")?.as_str()? != turn_id.simple().encode_lower(&mut [0; 32]) {
            return None;
        }
        Some(Self {
            turn_id,
            parent_span_id: if has_parent {
                Some(SpanId::parse(object["parent_span_id"].as_str()?)?)
            } else {
                None
            },
            ingress_request_id: uuid_v4(object.get("ingress_request_id")?.as_str()?)?,
        })
    }

    pub(crate) fn wire(self) -> Value {
        serde_json::to_value(Wire {
            version: 1,
            turn_id: self.turn_id.to_string(),
            trace_id: self.turn_id.simple().to_string(),
            parent_span_id: self.parent_span_id.map(|parent| parent.to_string()),
            ingress_request_id: self.ingress_request_id.to_string(),
        })
        .expect("fixed trace metadata is JSON serializable")
    }
}

/// Replay never replaces a canonical winner or turns a diagnostic conflict
/// into an execution error. The caller may count a conflict and keep delivering.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(from = "Value", into = "Value")]
pub(crate) struct AssociationSlot(Option<Association>);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BindResult {
    Bound,
    Duplicate,
    Conflict,
}

impl AssociationSlot {
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_none()
    }

    pub(crate) fn bind(&mut self, value: Association) -> BindResult {
        match self.0 {
            None => {
                self.0 = Some(value);
                BindResult::Bound
            }
            Some(existing) if existing == value => BindResult::Duplicate,
            Some(_) => BindResult::Conflict,
        }
    }

    pub(crate) fn get(self) -> Option<Association> {
        self.0
    }
}

impl From<Value> for AssociationSlot {
    fn from(value: Value) -> Self {
        // A malformed diagnostic annotation is a coverage gap. It cannot
        // invalidate an otherwise authentic execution or delivery checkpoint.
        Self(Association::retained(&value))
    }
}

impl From<AssociationSlot> for Value {
    fn from(slot: AssociationSlot) -> Self {
        slot.0.map_or(Value::Null, Association::wire)
    }
}

#[cfg(test)]
mod tests;
