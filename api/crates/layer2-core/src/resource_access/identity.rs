//! Durable attribution types; identity fields alone never establish authority.

use crate::{LaunchId, SubjectId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResourceOwner {
    Person { subject: SubjectId },
    Organization { subject: SubjectId },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResourceActor {
    Human {
        subject: SubjectId,
    },
    Api {
        subject: SubjectId,
        delegation_id: Option<String>,
    },
    Trigger {
        subject: SubjectId,
        launch_id: LaunchId,
    },
    Channel {
        bridge_id: String,
        grant_id: String,
        platform: String,
        chat_id: String,
        sender_id: String,
    },
}

impl ResourceActor {
    /// Structural attribution only. Authentication must verify any delegation
    /// before constructing a trusted CallerContext or authorizing execution.
    pub fn ordinary_subject(&self) -> Option<&SubjectId> {
        let subject = match self {
            Self::Human { subject } => subject,
            Self::Api {
                subject,
                delegation_id,
            } if delegation_id.as_ref().is_none_or(|id| {
                !id.is_empty()
                    && id.len() <= 256
                    && id.trim() == id
                    && !id.chars().any(char::is_control)
            }) =>
            {
                subject
            }
            _ => return None,
        };
        (!subject.as_str().trim().is_empty()).then_some(subject)
    }
}
