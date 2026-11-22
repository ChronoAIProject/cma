use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::ids::AgentCommandId;

use super::answer_deliverables::{
    AnswerDeliverableReading, DeliverableResource, DeliverableStatus,
};
use super::{AgentOperationError, AgentPlanStep};

const MAX_REQUIREMENTS: usize = 16;
const MAX_STEP_TITLE_BYTES: usize = 160;
const MAX_OUTPUT_PATH_BYTES: usize = 512;

pub(super) fn permits_admission(
    policy: &super::task_experience::TaskExperiencePolicy,
    context: &crate::resource_access::CallerContext,
    command: &super::agent_io::AgentCommandEnvelope,
) -> bool {
    command.command.task_acceptance().is_none()
        || (matches!(&command.owner, super::agent_io::AgentOwner::Person { subject } if subject == &context.caller.subject)
            && policy.allows(
                &context.caller.subject,
                super::task_experience::TaskExperienceCapability::TaskAcceptanceAdmission,
            ))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskAcceptanceIntent {
    pub version: u8,
    pub action: TaskAcceptanceAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum TaskAcceptanceAction {
    Declare {
        requirements: Vec<TaskRequirement>,
    },
    Retry {
        task_id: AgentCommandId,
        after_turn_sequence: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskRequirement {
    pub id: String,
    pub required: bool,
    pub evidence: TaskRequirementEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TaskRequirementEvidence {
    PlanStep { title: String },
    Deliverable { resource: DeliverableResource },
}

impl TaskAcceptanceIntent {
    pub(super) fn eligible(&self, previous: Option<&TaskAcceptance>) -> bool {
        match &self.action {
            TaskAcceptanceAction::Declare { .. } => {
                previous.is_none_or(|task| task.verdict == TaskAcceptanceVerdict::Succeeded)
            }
            TaskAcceptanceAction::Retry {
                task_id,
                after_turn_sequence,
            } => previous.is_some_and(|task| {
                &task.task_id == task_id
                    && task.turn_sequence == *after_turn_sequence
                    && matches!(
                        task.verdict,
                        TaskAcceptanceVerdict::NeedsAttention(_)
                            | TaskAcceptanceVerdict::Indeterminate(_)
                    )
            }),
        }
    }

    pub fn canonical(&self) -> Result<Self, AgentOperationError> {
        let invalid =
            || AgentOperationError::InvalidRequest("Invalid task acceptance contract".into());
        if self.version != 1 {
            return Err(invalid());
        }
        let mut intent = self.clone();
        match &mut intent.action {
            TaskAcceptanceAction::Declare { requirements } => {
                if requirements.is_empty() || requirements.len() > MAX_REQUIREMENTS {
                    return Err(invalid());
                }
                for requirement in requirements.iter() {
                    if !safe_id(&requirement.id) || !requirement.evidence.valid() {
                        return Err(invalid());
                    }
                }
                requirements.sort_by(|left, right| left.id.cmp(&right.id));
                for (index, item) in requirements.iter().enumerate() {
                    if requirements[..index].iter().any(|previous| {
                        previous.id == item.id || previous.evidence == item.evidence
                    }) {
                        return Err(invalid());
                    }
                }
            }
            TaskAcceptanceAction::Retry {
                task_id,
                after_turn_sequence,
            } => {
                if !safe_id(task_id.as_str()) || *after_turn_sequence == 0 {
                    return Err(invalid());
                }
            }
        }
        Ok(intent)
    }
}

impl TaskRequirementEvidence {
    fn valid(&self) -> bool {
        match self {
            Self::PlanStep { title } => {
                !title.is_empty()
                    && title.trim() == title
                    && title.len() <= MAX_STEP_TITLE_BYTES
                    && !title.chars().any(char::is_control)
            }
            Self::Deliverable { resource } => match resource {
                DeliverableResource::LiveFile { path } => {
                    !path.is_empty()
                        && path.len() <= MAX_OUTPUT_PATH_BYTES
                        && layer3_adapter::valid_agent_file_path(path)
                }
                DeliverableResource::AgentArtifact { id } => {
                    super::agent_artifacts::valid_artifact_id(id)
                }
                DeliverableResource::Document { id, revision } => {
                    crate::document_artifacts::valid_id(id, "doc_") && *revision > 0
                }
            },
        }
    }
}

fn safe_id(id: &str) -> bool {
    (1..=80).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", content = "reason", rename_all = "snake_case")]
pub enum TaskAcceptanceVerdict {
    InProgress,
    Succeeded,
    NeedsAttention(TaskAcceptanceReason),
    Indeterminate(TaskAcceptanceReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskAcceptanceReason {
    RequiredStepsIncomplete,
    RequiredDeliverablesMissing,
    Interrupted,
    ExecutionFailed,
    HistoryIncomplete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct VerifiedDelivery {
    answer_id: String,
    delivery_id: String,
    verified_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskAcceptance {
    version: u8,
    pub task_id: AgentCommandId,
    pub turn_sequence: u64,
    pub verdict: TaskAcceptanceVerdict,
    requirements: Vec<TaskRequirement>,
    completed_steps: BTreeSet<String>,
    deliveries: BTreeMap<String, VerifiedDelivery>,
    history_incomplete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskAcceptanceSummary {
    pub task_id: AgentCommandId,
    pub turn_sequence: u64,
    pub verdict: TaskAcceptanceVerdict,
    pub unconfirmed_outputs: Vec<DeliverableResource>,
}

impl TaskAcceptance {
    pub fn summary(&self) -> TaskAcceptanceSummary {
        TaskAcceptanceSummary {
            task_id: self.task_id.clone(),
            turn_sequence: self.turn_sequence,
            verdict: self.verdict,
            unconfirmed_outputs: self
                .requirements
                .iter()
                .filter_map(|item| match &item.evidence {
                    TaskRequirementEvidence::Deliverable { resource }
                        if item.required && !self.deliveries.contains_key(&item.id) =>
                    {
                        Some(resource.clone())
                    }
                    _ => None,
                })
                .collect(),
        }
    }

    pub(super) fn observe_execution(
        &mut self,
        active_turn: Option<u64>,
        logs: &[super::AgentLogKind],
    ) {
        use super::AgentLogKind;
        for log in logs {
            match log {
                AgentLogKind::PlanSet { entries } if active_turn == Some(self.turn_sequence) => {
                    self.observe_plan(self.turn_sequence, entries);
                }
                AgentLogKind::ConversationGap { .. } if active_turn == Some(self.turn_sequence) => {
                    self.observe_gap(self.turn_sequence);
                }
                AgentLogKind::TurnCompleted { turn_sequence } => {
                    self.conclude(*turn_sequence, None)
                }
                AgentLogKind::TurnFailed { turn_sequence, .. } => {
                    self.conclude(*turn_sequence, Some(TaskAcceptanceReason::ExecutionFailed));
                }
                AgentLogKind::TurnInterrupted { turn_sequence } => {
                    self.conclude(*turn_sequence, Some(TaskAcceptanceReason::Interrupted));
                }
                _ => (),
            }
        }
    }

    pub(super) fn bind(
        previous: Option<&Self>,
        intent: &TaskAcceptanceIntent,
        command_id: &AgentCommandId,
        turn_sequence: u64,
    ) -> Result<Self, AgentOperationError> {
        if !intent.eligible(previous) {
            return Err(AgentOperationError::Conflict);
        }
        let (task_id, requirements) = match intent.canonical()?.action {
            TaskAcceptanceAction::Declare { requirements } => (command_id.clone(), requirements),
            TaskAcceptanceAction::Retry { .. } => {
                let task = previous.ok_or(AgentOperationError::Conflict)?;
                (task.task_id.clone(), task.requirements.clone())
            }
        };
        Ok(Self {
            version: intent.version,
            task_id,
            turn_sequence,
            verdict: TaskAcceptanceVerdict::InProgress,
            requirements,
            completed_steps: BTreeSet::new(),
            deliveries: BTreeMap::new(),
            history_incomplete: false,
        })
    }

    pub(super) fn observe_plan(&mut self, turn: u64, plan: &[AgentPlanStep]) {
        if self.turn_sequence != turn || self.verdict != TaskAcceptanceVerdict::InProgress {
            return;
        }
        self.completed_steps = self
            .requirements
            .iter()
            .filter_map(|requirement| {
                let TaskRequirementEvidence::PlanStep { title } = &requirement.evidence else {
                    return None;
                };
                let mut matches = plan.iter().filter(|entry| entry.title == *title);
                (matches
                    .next()
                    .is_some_and(|entry| entry.status == "completed")
                    && matches.next().is_none())
                .then(|| requirement.id.clone())
            })
            .collect();
    }

    pub(super) fn observe_gap(&mut self, turn: u64) {
        if self.turn_sequence == turn && self.verdict == TaskAcceptanceVerdict::InProgress {
            self.history_incomplete = true;
        }
    }

    pub(super) fn conclude(&mut self, turn: u64, failure: Option<TaskAcceptanceReason>) {
        if self.turn_sequence != turn || self.verdict != TaskAcceptanceVerdict::InProgress {
            return;
        }
        self.verdict = match failure {
            Some(reason) => TaskAcceptanceVerdict::NeedsAttention(reason),
            None => self.evaluate(),
        };
    }

    pub(super) fn accept_delivery(&mut self, reading: &AnswerDeliverableReading, at: &str) -> bool {
        use TaskAcceptanceReason::{RequiredDeliverablesMissing, RequiredStepsIncomplete};
        if self.turn_sequence != reading.turn_sequence
            || !matches!(
                self.verdict,
                TaskAcceptanceVerdict::NeedsAttention(
                    RequiredDeliverablesMissing | RequiredStepsIncomplete
                )
            )
            || !matches!(
                reading.item.status,
                DeliverableStatus::Ready | DeliverableStatus::VersionChanged
            )
        {
            return false;
        }
        let Some(required) = self.requirements.iter().find(|item| {
            item.required && matches!(&item.evidence, TaskRequirementEvidence::Deliverable { resource } if resource == &reading.item.reference.resource)
        }) else { return false; };
        if self.deliveries.contains_key(&required.id) {
            return false;
        }
        self.deliveries.insert(
            required.id.clone(),
            VerifiedDelivery {
                answer_id: reading.answer_id.clone(),
                delivery_id: reading.item.id.clone(),
                verified_at: at.into(),
            },
        );
        self.verdict = self.evaluate();
        true
    }

    pub(super) fn confirmed(&self, reading: &AnswerDeliverableReading) -> bool {
        self.turn_sequence == reading.turn_sequence
            && self.deliveries.values().any(|receipt| {
                receipt.answer_id == reading.answer_id && receipt.delivery_id == reading.item.id
            })
    }

    fn evaluate(&self) -> TaskAcceptanceVerdict {
        use TaskAcceptanceReason::*;
        use TaskAcceptanceVerdict::*;
        if self.history_incomplete {
            return Indeterminate(HistoryIncomplete);
        }
        for required in self.requirements.iter().filter(|item| item.required) {
            if matches!(required.evidence, TaskRequirementEvidence::PlanStep { .. })
                && !self.completed_steps.contains(&required.id)
            {
                return NeedsAttention(RequiredStepsIncomplete);
            }
        }
        for required in self.requirements.iter().filter(|item| item.required) {
            if matches!(
                required.evidence,
                TaskRequirementEvidence::Deliverable { .. }
            ) && !self.deliveries.contains_key(&required.id)
            {
                return NeedsAttention(RequiredDeliverablesMissing);
            }
        }
        Succeeded
    }
}
