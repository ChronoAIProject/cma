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
