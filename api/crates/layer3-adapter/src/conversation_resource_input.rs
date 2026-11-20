use std::{
    io::Write,
    sync::{Arc, OnceLock},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::sync::Semaphore;

use crate::ingress_budget::{IngressBudget, IngressReservation, SharedCustody};
use crate::resource_execution_protocol::{
    Digest, ExecutionOperation, ExecutionTicketV1, ProtocolError, ProtocolResult,
    MAX_MESSAGE_BYTES, PROTOCOL_VERSION,
};
use crate::{ContentBlock, ConversationInput, PermissionDecision, PermissionLocator};

const INPUT_BYTES: usize = MAX_MESSAGE_BYTES / 2;
const INPUT_POOL_BYTES: usize = 32 * 1024 * 1024;
// Covers the bounded identity copy/validator and serialization diagnostics,
// before the encoding size is known and its separate allowance is acquired.
const VALIDATION_BYTES: usize = 4096;

/// Immutable semantic input. Its byte and parsed-shape custody follows clones.
#[derive(Clone)]
pub struct ResourceInput(SharedCustody<Storage>);

struct Storage {
    bytes: Vec<u8>,
    operation: ExecutionOperation,
    digest: Digest,
    prompt_id: Option<String>,
    budget: Budget,
    _custody: IngressReservation,
}

struct EncodingWork {
    bytes: Vec<u8>,
    _custody: IngressReservation,
}

impl std::fmt::Debug for ResourceInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResourceInput")
            .field("operation", &self.0.operation)
            .finish_non_exhaustive()
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InputV1 {
    protocol: u32,
    input: SemanticInput,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum SemanticInput {
    Prompt {
        content: Vec<ContentBlock>,
    },
    RecoverablePrompt {
        content: Vec<ContentBlock>,
        prompt_id: String,
    },
    Steer {
        content: Vec<ContentBlock>,
    },
    PermissionAnswer {
        id: String,
        locator: Option<PermissionLocator>,
        decision: Decision,
    },
    Configure {
        model: String,
        reasoning_effort: crate::ReasoningEffort,
        permission: crate::PermissionLevel,
        system_prompt: Option<String>,
    },
    ContextChange {
        service_origin: Option<String>,
        operation_sequence: u64,
        readiness_receipt: String,
        previous_guidance: Vec<String>,
        working_directory: Option<String>,
        guidance: String,
        model: String,
        reasoning_effort: crate::ReasoningEffort,
        permission: crate::PermissionLevel,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Decision {
    Allow,
    Deny,
    Abort,
}

impl SemanticInput {
    fn operation(&self) -> ExecutionOperation {
        match self {
            Self::Prompt { .. } => ExecutionOperation::Prompt,
            Self::RecoverablePrompt { .. } => ExecutionOperation::RecoverablePrompt,
            Self::Steer { .. } => ExecutionOperation::Steer,
            Self::PermissionAnswer { .. } => ExecutionOperation::PermissionAnswer,
            Self::Configure { .. } => ExecutionOperation::Configure,
            Self::ContextChange { .. } => ExecutionOperation::ContextChange,
        }
    }
}

impl TryFrom<ConversationInput> for SemanticInput {
    type Error = ProtocolError;
    fn try_from(input: ConversationInput) -> ProtocolResult<Self> {
        Ok(match input {
            ConversationInput::Prompt(content) => Self::Prompt { content },
            ConversationInput::RecoverablePrompt { content, prompt_id } => {
                Self::RecoverablePrompt {
                    content,
                    prompt_id: prompt_id.as_str().to_owned(),
                }
            }
            ConversationInput::Steer(content) => Self::Steer { content },
            ConversationInput::PermissionAnswer {
                id,
                locator,
                decision,
            } => Self::PermissionAnswer {
                id,
                locator,
                decision: match decision {
                    PermissionDecision::Allow => Decision::Allow,
                    PermissionDecision::Deny => Decision::Deny,
                    PermissionDecision::Abort => Decision::Abort,
                },
            },
            ConversationInput::Configure {
                model,
                reasoning_effort,
                permission,
                system_prompt,
            } => Self::Configure {
                model,
                reasoning_effort,
                permission,
                system_prompt,
            },
            ConversationInput::RefreshContext {
                service_origin,
                operation_sequence,
                readiness_receipt,
                previous_guidance,
                working_directory,
                guidance,
                model,
                reasoning_effort,
                permission,
            } => Self::ContextChange {
                service_origin,
                operation_sequence,
                readiness_receipt,
                previous_guidance,
                working_directory,
                guidance,
                model,
                reasoning_effort,
                permission,
            },
            ConversationInput::Cancel => return Err(ProtocolError::Unsupported),
        })
    }
}

impl ResourceInput {
    pub fn admit(input: ConversationInput) -> ProtocolResult<Self> {
        Self::admit_with(input, Budget::process())
    }

    fn admit_with(input: ConversationInput, budget: Budget) -> ProtocolResult<Self> {
        let _validation = budget.reserve(VALIDATION_BYTES)?;
        let input = InputV1 {
            protocol: PROTOCOL_VERSION,
            input: input.try_into()?,
        };
        // Counting and conversion diagnostics retain the validation allowance.
        let mut count = LimitedWriter {
            bytes: None,
            written: 0,
        };
        serde_json::to_writer(&mut count, &input).map_err(|_| ProtocolError::Capacity)?;
        let scratch = budget.reserve(count.written.saturating_mul(4) + 4096)?;
        let mut encoded = LimitedWriter {
            bytes: Some(Vec::with_capacity(count.written)),
            written: 0,
        };
        serde_json::to_writer(&mut encoded, &input).map_err(|_| ProtocolError::Capacity)?;
        Self::normalize(
            EncodingWork {
                bytes: encoded.bytes.expect("encoding writer"),
                _custody: scratch,
            },
            budget,
        )
    }

    /// Decode only bounded semantic input; raw ACP frames are not accepted.
    /// Noncanonical encodings are refused so every accepted digest has one encoding.
    pub fn decode(bytes: &[u8]) -> ProtocolResult<Self> {
        if bytes.len() > INPUT_BYTES {
            return Err(ProtocolError::Capacity);
        }
        let budget = Budget::process();
        let scratch = budget.reserve(bytes.len().saturating_mul(4) + 4096)?;
        let input = Self::normalize(
            EncodingWork {
                bytes: bytes.to_vec(),
                _custody: scratch,
            },
            budget,
        )?;
        if input.bytes() != bytes {
            return Err(ProtocolError::EvidenceMismatch);
        }
        Ok(input)
    }

    fn normalize(work: EncodingWork, budget: Budget) -> ProtocolResult<Self> {
        let shape = crate::hosted::output_json_budget::inspect(&work.bytes)
            .map_err(|_| ProtocolError::InvalidIdentity)?;
        let custody = budget.reserve(
            work.bytes.len().saturating_mul(8) + shape.nodes.saturating_mul(2048) + 4096,
        )?;
        let decoded: InputV1 =
            serde_json::from_slice(&work.bytes).map_err(|_| ProtocolError::InvalidIdentity)?;
        if decoded.protocol != PROTOCOL_VERSION {
            return Err(ProtocolError::Unsupported);
        }
        if let SemanticInput::RecoverablePrompt { prompt_id, .. } = &decoded.input {
            if crate::PromptIdentity::parse(prompt_id).is_none() {
                return Err(ProtocolError::InvalidIdentity);
            }
        }
        let operation = decoded.input.operation();
        let prompt_id = match &decoded.input {
            SemanticInput::RecoverablePrompt { prompt_id, .. } => Some(prompt_id.clone()),
            _ => None,
        };
        let mut value =
            serde_json::to_value(decoded).map_err(|_| ProtocolError::InvalidIdentity)?;
        value.sort_all_objects();
        let mut encoded = LimitedWriter {
            bytes: Some(Vec::with_capacity(work.bytes.len())),
            written: 0,
        };
        serde_json::to_writer(&mut encoded, &value).map_err(|_| ProtocolError::Capacity)?;
        let bytes = encoded.bytes.expect("encoding writer");
        let mut hash = Sha256::new();
        hash.update(b"cma-resource-input-v1\0");
        hash.update(&bytes);
        let digest = Digest::new(&format!("{:x}", hash.finalize()))?;
        Ok(Self(SharedCustody::new(Storage {
            bytes,
            operation,
            digest,
            prompt_id,
            budget,
            _custody: custody,
        })))
    }

    pub fn operation(&self) -> ExecutionOperation {
        self.0.operation
    }
    pub fn digest(&self) -> &Digest {
        &self.0.digest
    }
    pub fn bytes(&self) -> &[u8] {
        &self.0.bytes
    }

    pub fn bind(self, ticket: ExecutionTicketV1) -> ProtocolResult<ResourceDispatch> {
        let _validation = self.0.budget.reserve(VALIDATION_BYTES)?;
        ticket.validate()?;
        if ticket.operation != self.operation()
            || ticket.input_digest != *self.digest()
            || self
                .0
                .prompt_id
                .as_deref()
                .is_some_and(|id| id != ticket.execution.prompt_id.as_str())
        {
            return Err(ProtocolError::EvidenceMismatch);
        }
        let mut count = LimitedWriter {
            bytes: None,
            written: 0,
        };
        serde_json::to_writer(&mut count, &ticket).map_err(|_| ProtocolError::Capacity)?;
        let custody = self
            .0
            .budget
            .reserve(count.written.saturating_mul(4) + 4096)?;
        Ok(ResourceDispatch(SharedCustody::new(DispatchStorage {
            input: self,
            ticket,
            _custody: custody,
        })))
    }
}

/// Input cannot be replaced after its ticket has been checked.
#[derive(Clone)]
pub struct ResourceDispatch(SharedCustody<DispatchStorage>);

struct DispatchStorage {
    input: ResourceInput,
    ticket: ExecutionTicketV1,
    _custody: IngressReservation,
}

impl std::fmt::Debug for ResourceDispatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResourceDispatch")
            .field("operation", &self.input().operation())
            .finish_non_exhaustive()
    }
}

impl ResourceDispatch {
    pub fn input(&self) -> &ResourceInput {
        &self.0.input
    }
    pub fn ticket(&self) -> &ExecutionTicketV1 {
        &self.0.ticket
    }
}

#[derive(Clone)]
struct Budget {
    component: Arc<Semaphore>,
    aggregate: IngressBudget,
}

impl Budget {
    fn process() -> Self {
        static BUDGET: OnceLock<Budget> = OnceLock::new();
        BUDGET
            .get_or_init(|| Self {
                component: Arc::new(Semaphore::new(INPUT_POOL_BYTES)),
                aggregate: IngressBudget::process(),
            })
            .clone()
    }

    fn reserve(&self, allowance: usize) -> ProtocolResult<IngressReservation> {
        IngressReservation::try_acquire(&self.component, Some(&self.aggregate), allowance)
            .ok_or(ProtocolError::Capacity)
    }
}

struct LimitedWriter {
    bytes: Option<Vec<u8>>,
    written: usize,
}
impl Write for LimitedWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > INPUT_BYTES.saturating_sub(self.written) {
            return Err(std::io::Error::other("resource input size limit"));
        }
        self.written += bytes.len();
        if let Some(output) = &mut self.bytes {
            output.extend_from_slice(bytes);
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
#[path = "conversation_resource_input_allocation_tests.rs"]
mod allocation_tests;

#[cfg(test)]
mod custody_tests {
    use super::*;

    #[test]
    fn shared_input_retains_custody_until_its_last_owner_and_refuses_aggregate_pressure() {
        let budget = Budget {
            component: Arc::new(Semaphore::new(65536)),
            aggregate: IngressBudget::new(65536),
        };
        let input =
            ResourceInput::admit_with(ConversationInput::Prompt(vec![]), budget.clone()).unwrap();
        let shared = input.clone();
        let held = budget.aggregate.available_bytes();
        assert!(held < 65536);
        drop(input);
        assert_eq!(budget.aggregate.available_bytes(), held);
        let component = budget.component.available_permits();
        let external = Arc::new(Semaphore::new(65536));
        let pressure =
            IngressReservation::try_acquire(&external, Some(&budget.aggregate), held).unwrap();
        assert!(matches!(
            ResourceInput::admit_with(ConversationInput::Prompt(vec![]), budget.clone()),
            Err(ProtocolError::Capacity)
        ));
        assert_eq!(budget.component.available_permits(), component);
        assert_eq!(budget.aggregate.available_bytes(), 0);
        drop(pressure);
        drop(shared);
        assert_eq!(budget.aggregate.available_bytes(), 65536);
        assert_eq!(budget.component.available_permits(), 65536);
    }
}

#[cfg(test)]
#[path = "conversation_resource_input_deallocation_tests.rs"]
mod deallocation_tests;
