use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::super::{
    AgentApprovalDecision, AgentOperationError, AgentOperationIntent, AgentOperationKind,
};
use super::input::invalid;
use super::AgentInput;

/// An explicit predecessor is a linear-history condition. It is independent
/// from optimistic resource revision checks and durable queue positions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentInputOrder {
    #[default]
    Append,
    AfterTurn {
        turn_sequence: Option<u64>,
    },
}

/// Client-supplied intent only. No authority fields can be deserialized here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentCommand {
    Message {
        input: AgentInput,
        #[serde(default)]
        order: AgentInputOrder,
        #[serde(default)]
        metadata: BTreeMap<String, String>,
    },
    Steer {
        input: AgentInput,
    },
    Interrupt,
    AnswerApproval {
        approval_id: String,
        decision: AgentApprovalDecision,
    },
}

impl AgentCommand {
    pub fn message(text: impl Into<String>) -> Self {
        Self::Message {
            input: AgentInput::text(text),
            order: AgentInputOrder::Append,
            metadata: BTreeMap::new(),
        }
    }

    pub const fn kind(&self) -> AgentOperationKind {
        match self {
            Self::Message { .. } => AgentOperationKind::SendMessage,
            Self::Steer { .. } => AgentOperationKind::Steer,
            Self::Interrupt => AgentOperationKind::Interrupt,
            Self::AnswerApproval { .. } => AgentOperationKind::AnswerApproval,
        }
    }

    pub fn canonical(&self) -> Result<Self, AgentOperationError> {
        Ok(match self {
            Self::Message {
                input,
                order,
                metadata,
            } => {
                if metadata.len() > 16
                    || metadata
                        .iter()
                        .any(|(key, value)| key.chars().count() > 64 || value.chars().count() > 512)
                {
                    return Err(invalid("turn metadata is invalid"));
                }
                if matches!(
                    order,
                    AgentInputOrder::AfterTurn {
                        turn_sequence: Some(0)
                    }
                ) {
                    return Err(invalid("previous turn sequence is invalid"));
                }
                Self::Message {
                    input: input.canonical()?,
                    order: *order,
                    metadata: metadata.clone(),
                }
            }
            Self::Steer { input } => Self::Steer {
                input: input.canonical()?,
            },
            Self::Interrupt => Self::Interrupt,
            Self::AnswerApproval {
                approval_id,
                decision,
            } => {
                AgentOperationIntent::answer_approval(approval_id.clone(), *decision)
                    .map_err(|_| invalid("approval identity is invalid"))?;
                self.clone()
            }
        })
    }

    /// Stable across object-key order and equivalent adjacent text partitions.
    /// Attribution, credentials and transport timestamps are outside the input.
    pub fn fingerprint(&self) -> Result<String, AgentOperationError> {
        let bytes = serde_json::to_vec(&self.canonical()?)
            .map_err(|_| invalid("command cannot be encoded"))?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }
}
