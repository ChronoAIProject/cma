//! Resource-neutral durable event envelope.

use serde::{Deserialize, Serialize};

use crate::{now_rfc3339, CoreError};

pub use crate::event_kinds::{EventKind, PressureDimension};

/// One ordered event owned by an opaque resource identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Event {
    pub resource_id: String,
    pub sequence: u64,
    pub at: String,
    pub kind: EventKind,
}

impl Event {
    pub fn new(
        resource_id: impl Into<String>,
        sequence: u64,
        kind: EventKind,
    ) -> Result<Self, CoreError> {
        let resource_id = resource_id.into();
        if resource_id.trim().is_empty() || sequence == 0 {
            return Err(CoreError::InvalidRequest(
                "an event requires a resource id and positive sequence".into(),
            ));
        }
        Ok(Self {
            resource_id,
            sequence,
            at: now_rfc3339(),
            kind,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_and_round_trips_an_event() {
        let event = Event::new("resource-a", 1, EventKind::Created).unwrap();
        let encoded = serde_json::to_string(&event).unwrap();
        assert_eq!(serde_json::from_str::<Event>(&encoded).unwrap(), event);
        assert!(Event::new("", 1, EventKind::Deleted).is_err());
        assert!(Event::new("resource-a", 0, EventKind::Deleted).is_err());
    }
}
