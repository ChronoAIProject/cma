use super::task_acceptance::*;
use super::{
    agent_io::AgentCommand,
    answer_deliverables::{
        AnswerDeliverable, AnswerDeliverableReading, DeliverableReference, DeliverableResource,
        DeliverableStatus,
    },
    AgentLogKind, AgentPlanStep,
};
use crate::{ids::AgentCommandId, AgentId};

const TURN: u64 = 4;

fn required_file() -> DeliverableResource {
    DeliverableResource::LiveFile {
        path: "out/report.csv".into(),
    }
}

fn declaration() -> TaskAcceptanceIntent {
    TaskAcceptanceIntent {
        version: 1,
        action: TaskAcceptanceAction::Declare {
            requirements: vec![
                TaskRequirement {
                    id: "plan".into(),
                    required: true,
                    evidence: TaskRequirementEvidence::PlanStep {
                        title: "Write report".into(),
                    },
                },
                TaskRequirement {
                    id: "report".into(),
                    required: true,
                    evidence: TaskRequirementEvidence::Deliverable {
                        resource: required_file(),
                    },
                },
                TaskRequirement {
                    id: "optional".into(),
                    required: false,
                    evidence: TaskRequirementEvidence::PlanStep {
                        title: "Write summary".into(),
                    },
                },
            ],
        },
    }
}

fn bind() -> TaskAcceptance {
    TaskAcceptance::bind(
        None,
        &declaration(),
        &AgentCommandId("cmd_exact".into()),
        TURN,
    )
    .unwrap()
}

fn plan() -> AgentLogKind {
    AgentLogKind::PlanSet {
        entries: vec![AgentPlanStep {
            title: "Write report".into(),
            status: "completed".into(),
        }],
    }
}

fn receipt(turn_sequence: u64, resource: DeliverableResource) -> AnswerDeliverableReading {
    let agent_id = AgentId("agent_fixture".into());
    AnswerDeliverableReading {
        agent_id: agent_id.clone(),
        answer_id: "event:6".into(),
        turn_sequence,
        item: AnswerDeliverable {
            id: format!("delivery_{}", "a".repeat(64)),
            status: DeliverableStatus::Ready,
            reference: DeliverableReference {
                agent_id,
                resource,
                name: "report.csv".into(),
                media_type: "application/octet-stream".into(),
                size_bytes: 5,
                sha256: "b".repeat(64),
            },
        },
    }
}

#[test]
fn task_acceptance_remains_pending_until_the_exact_required_delivery_is_verified() {
    let mut task = bind();
    task.observe_execution(
        Some(TURN),
        &[
            plan(),
            AgentLogKind::TurnCompleted {
                turn_sequence: TURN,
            },
        ],
    );
    assert_eq!(
        task.verdict,
        TaskAcceptanceVerdict::NeedsAttention(TaskAcceptanceReason::RequiredDeliverablesMissing)
    );
    let persisted = serde_json::to_vec(&task).unwrap();
    let mut task: TaskAcceptance = serde_json::from_slice(&persisted).unwrap();
    let at = "2026-11-21T00:00:00Z";
    assert!(!task.accept_delivery(&receipt(TURN + 1, required_file()), at));
    assert!(!task.accept_delivery(
        &receipt(
            TURN,
            DeliverableResource::LiveFile {
                path: "other/report.csv".into()
            }
        ),
        at
    ));
    let exact = receipt(TURN, required_file());
    let mut changed = exact.clone();
    changed.item.status = DeliverableStatus::Changed;
    assert!(!task.accept_delivery(&changed, at));
    assert!(task.accept_delivery(&exact, at));
    assert!(task.confirmed(&exact));
    assert!(!task.accept_delivery(&exact, at));
    assert_eq!(task.verdict, TaskAcceptanceVerdict::Succeeded);
    assert!(task.summary().unconfirmed_outputs.is_empty());
}

#[test]
fn task_acceptance_does_not_promote_unobserved_or_ambiguous_plan_steps() {
    let mut task = bind();
    task.observe_execution(
        Some(TURN - 1),
        &[
            plan(),
            AgentLogKind::TurnCompleted {
                turn_sequence: TURN - 1,
            },
        ],
    );
    assert_eq!(task.verdict, TaskAcceptanceVerdict::InProgress);
    task.observe_execution(
        Some(TURN),
        &[AgentLogKind::TurnCompleted {
            turn_sequence: TURN,
        }],
    );
    assert_eq!(
        task.verdict,
        TaskAcceptanceVerdict::NeedsAttention(TaskAcceptanceReason::RequiredStepsIncomplete)
    );
    task.observe_execution(
        None,
        &[
            plan(),
            AgentLogKind::TurnCompleted {
                turn_sequence: TURN + 1,
            },
        ],
    );
    assert_eq!(
        task.verdict,
        TaskAcceptanceVerdict::NeedsAttention(TaskAcceptanceReason::RequiredStepsIncomplete)
    );
}

#[test]
fn task_acceptance_preserves_execution_failures_and_allows_one_explicit_retry() {
    for (terminal, expected) in [
        (
            AgentLogKind::TurnInterrupted {
                turn_sequence: TURN,
            },
            TaskAcceptanceVerdict::NeedsAttention(TaskAcceptanceReason::Interrupted),
        ),
        (
            AgentLogKind::TurnCompleted {
                turn_sequence: TURN,
            },
            TaskAcceptanceVerdict::Indeterminate(TaskAcceptanceReason::HistoryIncomplete),
        ),
    ] {
        let mut task = bind();
        task.observe_execution(
            Some(TURN),
            &[
                plan(),
                AgentLogKind::ConversationGap { frames: 1 },
                terminal,
            ],
        );
        assert_eq!(task.verdict, expected);
        let intent = TaskAcceptanceIntent {
            version: 1,
            action: TaskAcceptanceAction::Retry {
                task_id: task.task_id.clone(),
                after_turn_sequence: TURN,
            },
        };
        let command = AgentCommandId("cmd_retry".into());
        let resumed = TaskAcceptance::bind(Some(&task), &intent, &command, TURN + 8).unwrap();
        assert_eq!(resumed.task_id, task.task_id);
        assert_eq!(resumed.summary().unconfirmed_outputs, vec![required_file()]);
        assert!(TaskAcceptance::bind(Some(&resumed), &intent, &command, TURN + 9).is_err());
    }
}

#[test]
fn task_acceptance_is_part_of_the_canonical_command_fingerprint() {
    let command = |intent: Option<TaskAcceptanceIntent>| {
        let mut command = AgentCommand::message("Prepare the report");
        if let AgentCommand::Message { input, .. } = &mut command {
            input.task_acceptance = intent;
        }
        command.fingerprint().unwrap()
    };
    let initial = declaration();
    let mut changed = initial.clone();
    let TaskAcceptanceAction::Declare { requirements } = &mut changed.action else {
        unreachable!()
    };
    requirements.reverse();
    assert_eq!(
        command(Some(initial.clone())),
        command(Some(changed.clone()))
    );
    let TaskAcceptanceAction::Declare { requirements } = &mut changed.action else {
        unreachable!()
    };
    requirements[0].required = true;
    assert_ne!(command(Some(initial.clone())), command(Some(changed)));
    assert_ne!(command(Some(initial)), command(None));
}
