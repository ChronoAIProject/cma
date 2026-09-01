//! Owned prompt/resume admission and its finite durable replay outcome.

use std::{sync::Arc, time::Duration};

use layer2_core::{
    ApprovalDecision, ApprovalResolution, ConversationReadinessError, CoreError, OperationStore,
    Session, SessionCapability, SessionId, SessionService, SessionStatus,
};
use serde::{Deserialize, Serialize};

use super::admission::{
    self, AcquiredClaim, Reservation, RunFailure, StoredFailure, StoredProblem,
};

const RUN_OPERATION_PREFIX: &str = "agui-run-admission-v1";
const CONVERSATION_READINESS_BOUND: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone)]
pub(super) enum RunAction {
    Prompt(String),
    Resume(Vec<ApprovalResolution>),
}

impl RunAction {
    pub(super) fn is_prompt(&self) -> bool {
        matches!(self, Self::Prompt(_))
    }

    pub(super) fn is_resume(&self) -> bool {
        matches!(self, Self::Resume(_))
    }
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum RunFingerprint<'a> {
    Prompt {
        text: &'a str,
    },
    Resume {
        resolutions: Vec<ResolutionFingerprint<'a>>,
    },
}

#[derive(Serialize)]
struct ResolutionFingerprint<'a> {
    interrupt_id: &'a str,
    decision: &'static str,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum RunAdmissionOutcome {
    PromptAccepted {
        turn_started: u64,
        synthetic_start: bool,
    },
    ResumeAccepted {
        after: u64,
    },
    Refused {
        failure: StoredFailure,
    },
}

impl RunAdmissionOutcome {
    fn synthetic_start(&self) -> bool {
        match self {
            Self::PromptAccepted {
                synthetic_start, ..
            } => *synthetic_start,
            Self::ResumeAccepted { .. } => true,
            Self::Refused { failure } => failure.streamed,
        }
    }
}

pub(super) enum AdmissionReport {
    Accepted {
        outcome: RunAdmissionOutcome,
        watching: Option<tokio::sync::broadcast::Receiver<layer2_core::LiveFrame>>,
        resources: Option<tokio::sync::broadcast::Receiver<layer2_core::ResourceReading>>,
    },
    Refused(StoredFailure),
}

pub(super) struct StreamPlan {
    pub(super) after: u64,
    pub(super) synthetic_start: bool,
    pub(super) watching: Option<tokio::sync::broadcast::Receiver<layer2_core::LiveFrame>>,
    pub(super) resources: Option<tokio::sync::broadcast::Receiver<layer2_core::ResourceReading>>,
    pub(super) admission: Option<tokio::task::JoinHandle<AdmissionReport>>,
    pub(super) failure: Option<StoredFailure>,
}

/// Reserve an explicit run id when present, then own admission independently
/// of the request or response body that is observing it.
pub(super) async fn prepare(
    service: Arc<SessionService>,
    operations: Arc<dyn OperationStore>,
    caller: layer2_core::Caller,
    session: Session,
    explicit_run_id: Option<&str>,
    action: RunAction,
) -> Result<StreamPlan, RunFailure> {
    let id = session.id;
    let reservation = match explicit_run_id {
        Some(key) => Some(
            admission::reserve::<RunAdmissionOutcome>(
                operations,
                &caller,
                format!("{RUN_OPERATION_PREFIX}:{}", id.0),
                key,
                run_fingerprint(&action)?,
            )
            .await?,
        ),
        // The generated wire id cannot identify a later retry. The task is
        // still owned, but this request makes no durable deduplication claim.
        None => None,
    };

    match reservation {
        Some(Reservation::Complete(outcome)) => replay_plan(&service, &caller, &id, outcome).await,
        claim => {
            let claim = match claim {
                Some(Reservation::Acquired(claim)) => Some(claim),
                None => None,
                Some(Reservation::Complete(_)) => unreachable!("handled above"),
            };
            let session = match service.session_for_interaction(&caller, &id).await {
                Ok(session) => session,
                Err(error) => {
                    return plan_from_report(
                        complete_refusal(core_problem(error).await, false, claim).await,
                    )
                }
            };
            if let Err(error) = session
                .capabilities
                .require(SessionCapability::Conversation)
            {
                return plan_from_report(
                    complete_refusal(core_problem(error).await, false, claim).await,
                );
            }
            let status = session.status;
            let capabilities = session.capabilities;
            let paused_chat = action.is_prompt()
                && status == SessionStatus::Idle
                && session.stop_reason == Some(layer2_core::StopReason::IdleTimeout)
                && session.chat.is_some();
            let deferred =
                action.is_prompt() && (status == SessionStatus::Provisioning || paused_chat);
            if action.is_prompt() && status == SessionStatus::Running {
                if let Err(error) = capabilities.require(SessionCapability::Steer) {
                    let problem = core_problem(error).await;
                    return plan_from_report(complete_refusal(problem, false, claim).await);
                }
            }
            let admission =
                spawn_admission(service, caller, id, action, deferred, paused_chat, claim);
            if deferred {
                Ok(StreamPlan {
                    after: 0,
                    synthetic_start: true,
                    watching: None,
                    resources: None,
                    admission: Some(admission),
                    failure: None,
                })
            } else {
                plan_from_report(await_admission(admission, false).await)
            }
        }
    }
}

/// Observe a deferred owned task. Dropping its `JoinHandle` only detaches it.
pub(super) async fn settle_deferred(
    admission: tokio::task::JoinHandle<AdmissionReport>,
) -> Result<StreamPlan, RunFailure> {
    plan_from_report(await_admission(admission, true).await)
}

fn run_fingerprint(action: &RunAction) -> Result<[u8; 32], RunFailure> {
    match action {
        RunAction::Prompt(text) => admission::fingerprint(&RunFingerprint::Prompt { text }),
        RunAction::Resume(resolutions) => admission::fingerprint(&RunFingerprint::Resume {
            resolutions: resolutions
                .iter()
                .map(|resolution| ResolutionFingerprint {
                    interrupt_id: &resolution.request_id,
                    decision: decision_name(resolution.decision),
                })
                .collect(),
        }),
    }
}

fn decision_name(decision: ApprovalDecision) -> &'static str {
    match decision {
        ApprovalDecision::Allow => "allow",
        ApprovalDecision::Deny => "deny",
        ApprovalDecision::Abort => "abort",
    }
}

fn spawn_admission(
    service: Arc<SessionService>,
    caller: layer2_core::Caller,
    id: SessionId,
    action: RunAction,
    deferred: bool,
    paused_chat: bool,
    claim: Option<AcquiredClaim>,
) -> tokio::task::JoinHandle<AdmissionReport> {
    tokio::spawn(async move {
        perform_admission(service, caller, id, action, deferred, paused_chat, claim).await
    })
}

async fn perform_admission(
    service: Arc<SessionService>,
    caller: layer2_core::Caller,
    id: SessionId,
    action: RunAction,
    deferred: bool,
    paused_chat: bool,
    claim: Option<AcquiredClaim>,
) -> AdmissionReport {
    if action.is_prompt() && !paused_chat {
        if let Err(error) = service
            .await_conversable(&caller, &id, CONVERSATION_READINESS_BOUND)
            .await
        {
            let problem = match error {
                ConversationReadinessError::ProvisioningTimedOut if deferred => {
                    StoredProblem::provisioning_timeout()
                }
                ConversationReadinessError::ProvisioningTimedOut => {
                    core_problem(CoreError::RuntimeUnavailable(
                        "the conversation did not become reachable before the readiness deadline"
                            .into(),
                    ))
                    .await
                }
                ConversationReadinessError::Refused(error) => core_problem(error).await,
            };
            return complete_refusal(problem, deferred, claim).await;
        }
    }

    // Subscribe after readiness established the live entry and before sending
    // input, so an immediate runtime response cannot outrun its receiver.
    let watching = service.subscribe_live(&caller, &id).await.ok();
    let resources = service.subscribe_resources(&caller, &id).await.ok();
    let outcome = match action {
        RunAction::Prompt(prompt) => match tokio::time::timeout(
            CONVERSATION_READINESS_BOUND,
            service.send_prompt(&caller, &id, prompt),
        )
        .await
        .map_err(|_| ConversationReadinessError::ProvisioningTimedOut)
        .and_then(|result| result.map_err(ConversationReadinessError::Refused))
        {
            Ok(turn_started) => RunAdmissionOutcome::PromptAccepted {
                turn_started,
                synthetic_start: deferred,
            },
            Err(ConversationReadinessError::ProvisioningTimedOut) => {
                return complete_refusal(StoredProblem::provisioning_timeout(), deferred, claim)
                    .await
            }
            Err(ConversationReadinessError::Refused(error)) => {
                return complete_refusal(core_problem(error).await, deferred, claim).await;
            }
        },
        RunAction::Resume(resolutions) => {
            // A continuation has no durable TurnStarted of its own. Snapshot
            // before resolving so every durable effect remains after the
            // synthetic cursorless boundary.
            let after = match service.events_since(&caller, &id, None).await {
                Ok(events) => events.last().map(|event| event.seq).unwrap_or(0),
                Err(error) => {
                    return complete_refusal(core_problem(error).await, false, claim).await
                }
            };
            if let Err(error) = service.resolve_approvals(&caller, &id, &resolutions).await {
                return complete_refusal(core_problem(error).await, false, claim).await;
            }
            RunAdmissionOutcome::ResumeAccepted { after }
        }
    };

    if let Some(claim) = claim {
        if claim.complete(&outcome).await.is_err() {
            tracing::error!(
                session_id = %id,
                "AG-UI run was admitted but its operation outcome was not recorded"
            );
            return AdmissionReport::Refused(StoredFailure {
                problem: StoredProblem::completion_failed(),
                streamed: deferred,
            });
        }
    }
    AdmissionReport::Accepted {
        outcome,
        watching,
        resources,
    }
}

async fn complete_refusal(
    problem: StoredProblem,
    streamed: bool,
    claim: Option<AcquiredClaim>,
) -> AdmissionReport {
    let failure = StoredFailure { problem, streamed };
    if let Some(claim) = claim {
        if claim
            .complete(&RunAdmissionOutcome::Refused {
                failure: failure.clone(),
            })
            .await
            .is_err()
        {
            tracing::error!("AG-UI run refusal could not be recorded");
            return AdmissionReport::Refused(StoredFailure {
                problem: StoredProblem::completion_failed(),
                streamed,
            });
        }
    }
    AdmissionReport::Refused(failure)
}

async fn core_problem(error: CoreError) -> StoredProblem {
    StoredProblem::from_core(error)
        .await
        .unwrap_or_else(|_| StoredProblem::task_failed())
}

async fn await_admission(
    task: tokio::task::JoinHandle<AdmissionReport>,
    streamed: bool,
) -> AdmissionReport {
    match task.await {
        Ok(report) => report,
        Err(error) => {
            tracing::error!(%error, "owned AG-UI run admission task failed");
            AdmissionReport::Refused(StoredFailure {
                problem: StoredProblem::task_failed(),
                streamed,
            })
        }
    }
}

async fn replay_plan(
    service: &Arc<SessionService>,
    caller: &layer2_core::Caller,
    id: &SessionId,
    outcome: RunAdmissionOutcome,
) -> Result<StreamPlan, RunFailure> {
    match outcome {
        RunAdmissionOutcome::Refused { failure } => {
            plan_from_report(AdmissionReport::Refused(failure))
        }
        outcome => {
            let watching = service.subscribe_live(caller, id).await.ok();
            let resources = service.subscribe_resources(caller, id).await.ok();
            plan_from_report(AdmissionReport::Accepted {
                outcome,
                watching,
                resources,
            })
        }
    }
}

fn plan_from_report(report: AdmissionReport) -> Result<StreamPlan, RunFailure> {
    match report {
        AdmissionReport::Refused(failure) if !failure.streamed => {
            Err(RunFailure::Stored(failure.problem))
        }
        AdmissionReport::Refused(failure) => Ok(StreamPlan {
            after: 0,
            synthetic_start: true,
            watching: None,
            resources: None,
            admission: None,
            failure: Some(failure),
        }),
        AdmissionReport::Accepted {
            outcome,
            watching,
            resources,
        } => {
            let synthetic_start = outcome.synthetic_start();
            let after = match outcome {
                RunAdmissionOutcome::PromptAccepted {
                    turn_started,
                    synthetic_start: true,
                } => turn_started,
                RunAdmissionOutcome::PromptAccepted {
                    turn_started,
                    synthetic_start: false,
                } => turn_started
                    .checked_sub(1)
                    .ok_or_else(|| RunFailure::Stored(StoredProblem::task_failed()))?,
                RunAdmissionOutcome::ResumeAccepted { after } => after,
                RunAdmissionOutcome::Refused { .. } => unreachable!("handled above"),
            };
            Ok(StreamPlan {
                after,
                synthetic_start,
                watching,
                resources,
                admission: None,
                failure: None,
            })
        }
    }
}

#[cfg(test)]
#[path = "turn_admission_tests.rs"]
mod tests;
