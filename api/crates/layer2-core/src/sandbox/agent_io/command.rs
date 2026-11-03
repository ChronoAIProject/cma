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
    /// Reserved atomically with cancellation of this turn. These messages
    /// precede ordinary queued input without changing its admission order.
    InterruptAfterTurn {
        turn_sequence: u64,
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
        super::AnswerContext::from_command(self)
            .map_err(|_| invalid("answer context is invalid"))?;
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
                if matches!(order, AgentInputOrder::InterruptAfterTurn { turn_sequence }
                    if !(1..=i64::MAX as u64).contains(turn_sequence))
                {
                    return Err(invalid("interrupted turn sequence is invalid"));
                }
                Self::Message {
                    input: input.canonical()?,
                    order: *order,
                    metadata: metadata.clone(),
                }
            }
            Self::Steer { input } => {
                if input.external_event().is_some() {
                    return Err(invalid("external events require a message"));
                }
                Self::Steer {
                    input: input.canonical()?,
                }
            }
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
        let value = serde_json::to_value(self.canonical()?)
            .map_err(|_| invalid("command cannot be encoded"))?;
        let mut bytes = b"cma.agent-command.v1\n".to_vec();
        write_canonical(&value, &mut bytes).map_err(|_| invalid("command cannot be encoded"))?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }
}

// Explicit recursive key ordering makes the persisted fingerprint independent
// of Rust field declaration order and dependency feature unification.
pub(super) fn write_canonical(
    value: &serde_json::Value,
    output: &mut Vec<u8>,
) -> Result<(), serde_json::Error> {
    use serde_json::Value;
    match value {
        Value::Object(object) => {
            output.push(b'{');
            let mut entries: Vec<_> = object.iter().collect();
            entries.sort_unstable_by_key(|(key, _)| *key);
            for (index, (key, value)) in entries.into_iter().enumerate() {
                if index > 0 {
                    output.push(b',');
                }
                serde_json::to_writer(&mut *output, key)?;
                output.push(b':');
                write_canonical(value, output)?;
            }
            output.push(b'}');
        }
        Value::Array(values) => {
            output.push(b'[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push(b',');
                }
                write_canonical(value, output)?;
            }
            output.push(b']');
        }
        value => serde_json::to_writer(output, value)?,
    }
    Ok(())
}
