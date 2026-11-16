//! Private transport intents contain input, never a source-AM authorization snapshot.
use layer2_core::{sandbox::agent_io::*, AgentId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Intent {
    Admit {
        agent_id: AgentId,
        command: AgentCommand,
        idempotency_key: String,
        expected_agent_revision: Option<u64>,
        requested_model: Option<String>,
        binding_policy: BindingPolicy,
    },
    TargetedControl {
        agent_id: AgentId,
        target: TargetedResponse,
        expected_agent_revision: u64,
        command: AgentCommand,
        idempotency_key: String,
    },
    DirectTargetedControl {
        agent_id: AgentId,
        target: TargetedResponse,
        expected_agent_revision: u64,
        command: AgentCommand,
        idempotency_key: String,
    },
    InterruptingMessage {
        agent_id: AgentId,
        target: TargetedResponse,
        expected_agent_revision: u64,
        text: String,
        idempotency_key: String,
    },
    QueuedInput {
        agent_id: AgentId,
        target: QueuedCancellationTarget,
        text: String,
        idempotency_key: String,
    },
    QueuedCancellation {
        agent_id: AgentId,
        target: QueuedCancellationTarget,
        idempotency_key: String,
    },
}

// JSON null cannot distinguish None from Some(None). Use explicit variants.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "assertion", content = "revision", rename_all = "snake_case")]
pub(super) enum BindingPolicy {
    Unspecified,
    Absent,
    Revision(u64),
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Source {
    UserInterface,
    Api,
}

impl From<CommandSubmissionSource> for Source {
    fn from(value: CommandSubmissionSource) -> Self {
        match value {
            CommandSubmissionSource::UserInterface => Self::UserInterface,
            CommandSubmissionSource::Api => Self::Api,
        }
    }
}
impl From<Source> for CommandSubmissionSource {
    fn from(value: Source) -> Self {
        match value {
            Source::UserInterface => Self::UserInterface,
            Source::Api => Self::Api,
        }
    }
}
impl From<OwnerCommandMutation> for Intent {
    fn from(value: OwnerCommandMutation) -> Self {
        match value {
            OwnerCommandMutation::Admit {
                submission,
                binding_policy,
            } => Self::Admit {
                agent_id: submission.agent_id,
                command: submission.command,
                idempotency_key: submission.idempotency_key,
                expected_agent_revision: submission.expected_agent_revision,
                requested_model: submission.requested_model,
                binding_policy: match binding_policy {
                    None => BindingPolicy::Unspecified,
                    Some(None) => BindingPolicy::Absent,
                    Some(Some(revision)) => BindingPolicy::Revision(revision),
                },
            },
            OwnerCommandMutation::TargetedControl(value) => Self::TargetedControl {
                agent_id: value.agent_id,
                target: value.target,
                expected_agent_revision: value.expected_agent_revision,
                command: value.command,
                idempotency_key: value.idempotency_key,
            },
            OwnerCommandMutation::DirectTargetedControl(value) => Self::DirectTargetedControl {
                agent_id: value.agent_id,
                target: value.target,
                expected_agent_revision: value.expected_agent_revision,
                command: value.command,
                idempotency_key: value.idempotency_key,
            },
            OwnerCommandMutation::InterruptingMessage(value) => Self::InterruptingMessage {
                agent_id: value.agent_id,
                target: value.target,
                expected_agent_revision: value.expected_agent_revision,
                text: value.text,
                idempotency_key: value.idempotency_key,
            },
            OwnerCommandMutation::QueuedInput(value) => Self::QueuedInput {
                agent_id: value.agent_id,
                target: value.target,
                text: value.text,
                idempotency_key: value.idempotency_key,
            },
            OwnerCommandMutation::QueuedCancellation(value) => Self::QueuedCancellation {
                agent_id: value.agent_id,
                target: value.target,
                idempotency_key: value.idempotency_key,
            },
        }
    }
}
impl From<Intent> for OwnerCommandMutation {
    fn from(value: Intent) -> Self {
        match value {
            Intent::Admit {
                agent_id,
                command,
                idempotency_key,
                expected_agent_revision,
                requested_model,
                binding_policy,
            } => Self::Admit {
                submission: CommandSubmission {
                    agent_id,
                    command,
                    idempotency_key,
                    expected_agent_revision,
                    requested_model,
                },
                binding_policy: match binding_policy {
                    BindingPolicy::Unspecified => None,
                    BindingPolicy::Absent => Some(None),
                    BindingPolicy::Revision(value) => Some(Some(value)),
                },
            },
            Intent::TargetedControl {
                agent_id,
                target,
                expected_agent_revision,
                command,
                idempotency_key,
            } => Self::TargetedControl(TargetedControlSubmission {
                agent_id,
                target,
                expected_agent_revision,
                command,
                idempotency_key,
            }),
            Intent::DirectTargetedControl {
                agent_id,
                target,
                expected_agent_revision,
                command,
                idempotency_key,
            } => Self::DirectTargetedControl(TargetedControlSubmission {
                agent_id,
                target,
                expected_agent_revision,
                command,
                idempotency_key,
            }),
            Intent::InterruptingMessage {
                agent_id,
                target,
                expected_agent_revision,
                text,
                idempotency_key,
            } => Self::InterruptingMessage(InterruptingMessageSubmission {
                agent_id,
                target,
                expected_agent_revision,
                text,
                idempotency_key,
            }),
            Intent::QueuedInput {
                agent_id,
                target,
                text,
                idempotency_key,
            } => Self::QueuedInput(QueuedInputSubmission {
                agent_id,
                target,
                text,
                idempotency_key,
            }),
            Intent::QueuedCancellation {
                agent_id,
                target,
                idempotency_key,
            } => Self::QueuedCancellation(QueuedCancellationSubmission {
                agent_id,
                target,
                idempotency_key,
            }),
        }
    }
}
