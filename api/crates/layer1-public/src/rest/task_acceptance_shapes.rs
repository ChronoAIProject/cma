use layer2_core::sandbox::answer_deliverables::DeliverableResource;
use layer2_core::sandbox::task_acceptance::{
    TaskAcceptanceReason, TaskAcceptanceSummary, TaskAcceptanceVerdict,
};
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
#[allow(dead_code)]
pub(crate) struct TaskAcceptanceRequestView {
    #[schema(minimum = 1, maximum = 1)]
    version: u8,
    action: TaskAcceptanceActionView,
}

#[derive(Serialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(dead_code)]
enum TaskAcceptanceActionView {
    Declare {
        requirements: Vec<TaskRequirementView>,
    },
    Retry {
        task_id: String,
        after_turn_sequence: u64,
    },
}

#[derive(Serialize, ToSchema)]
struct TaskRequirementView {
    id: String,
    required: bool,
    evidence: TaskRequirementEvidenceView,
}

#[derive(Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[allow(dead_code)]
enum TaskRequirementEvidenceView {
    PlanStep {
        title: String,
    },
    Deliverable {
        #[schema(value_type = super::answer_deliverables::DeliverableResourceView)]
        resource: DeliverableResource,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub(crate) struct TaskAcceptanceView {
    task_id: String,
    turn_sequence: u64,
    verdict: TaskAcceptanceVerdictView,
    #[schema(value_type = Vec<super::answer_deliverables::DeliverableResourceView>)]
    unconfirmed_outputs: Vec<DeliverableResource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
enum TaskAcceptanceVerdictView {
    InProgress,
    Succeeded,
    NeedsAttention {
        #[schema(value_type = String)]
        reason: TaskAcceptanceReason,
    },
    Indeterminate {
        #[schema(value_type = String)]
        reason: TaskAcceptanceReason,
    },
}

impl From<TaskAcceptanceSummary> for TaskAcceptanceView {
    fn from(task: TaskAcceptanceSummary) -> Self {
        let verdict = match task.verdict {
            TaskAcceptanceVerdict::InProgress => TaskAcceptanceVerdictView::InProgress,
            TaskAcceptanceVerdict::Succeeded => TaskAcceptanceVerdictView::Succeeded,
            TaskAcceptanceVerdict::NeedsAttention(reason) => {
                TaskAcceptanceVerdictView::NeedsAttention { reason }
            }
            TaskAcceptanceVerdict::Indeterminate(reason) => {
                TaskAcceptanceVerdictView::Indeterminate { reason }
            }
        };
        Self {
            task_id: task.task_id.to_string(),
            turn_sequence: task.turn_sequence,
            verdict,
            unconfirmed_outputs: task.unconfirmed_outputs,
        }
    }
}
