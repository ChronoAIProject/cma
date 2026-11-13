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

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Wire {
    version: u8,
    turn_id: String,
    trace_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
        // Bound before cloning provider-controlled strings. A snapshot has its
        // own transport bound; this annotation has a much smaller closed shape.
        let object = value.as_object()?;
        let has_parent = object.contains_key("parent_span_id");
        if object.len() != if has_parent { 5 } else { 4 }
            || object.get("version")?.as_u64()? != 1
            || ["turn_id", "trace_id", "ingress_request_id"]
                .iter()
                .any(|key| {
                    object
                        .get(*key)
                        .and_then(Value::as_str)
                        .is_none_or(|v| v.len() > 36)
                })
        {
            return None;
        }
        if has_parent
            && object["parent_span_id"]
                .as_str()
                .is_none_or(|value| value.len() != 16)
        {
            return None;
        }
        let wire: Wire = serde_json::from_value(value.clone()).ok()?;
        let turn_id = uuid_v4(&wire.turn_id)?;
        if wire.trace_id != turn_id.simple().to_string() {
            return None;
        }
        Some(Self {
            turn_id,
            parent_span_id: match wire.parent_span_id {
                Some(parent) => Some(SpanId::parse(&parent)?),
                None => None,
            },
            ingress_request_id: uuid_v4(&wire.ingress_request_id)?,
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
