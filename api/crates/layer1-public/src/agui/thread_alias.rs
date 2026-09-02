//! Durable mapping from a requested fresh thread id to the core-minted session.

use std::sync::Arc;

use layer2_core::{CoreError, OperationStore, Session, SessionId, SessionService, SessionSpec};
use serde::{Deserialize, Serialize};

use crate::auth::DelegatedCredential;

use super::admission::{self, Reservation, RunFailure, StoredProblem};

const CREATION_OPERATION: &str = "agui-thread-create-v1";

#[derive(Serialize)]
struct CreationFingerprint<'a> {
    environment_profile: Option<&'a str>,
    surface: Option<layer2_core::SessionSurface>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum CreationOutcome {
    Created { effective_thread_id: String },
    Refused { problem: StoredProblem },
}

pub(super) struct CreationParameters {
    pub credential: Option<DelegatedCredential>,
    pub environment_profile: Option<String>,
    pub surface: Option<layer2_core::SessionSurface>,
}

/// Find an existing session or create one under a durable requested-id alias.
pub(super) async fn session_for_run(
    service: Arc<SessionService>,
    operations: Arc<dyn OperationStore>,
    caller: layer2_core::Caller,
    requested: SessionId,
    creation: CreationParameters,
    resume: bool,
) -> Result<Session, RunFailure> {
    let CreationParameters {
        credential,
        environment_profile,
        surface,
    } = creation;
    match service.get_session(&caller, &requested).await {
        Ok(session) => return Ok(session),
        // Anything else -- a foreign session or a store outage -- is not a
        // missing thread and must not be answered by creating one.
        Err(CoreError::SessionNotFound) => {}
        Err(error) => return Err(error.into()),
    }
    if resume {
        return Err(CoreError::InvalidRequest(
            "a run cannot resume a thread that does not exist".into(),
        )
        .into());
    }

    let fingerprint = admission::fingerprint(&CreationFingerprint {
        environment_profile: environment_profile.as_deref(),
        surface,
    })?;
    match admission::reserve::<CreationOutcome>(
        operations,
        &caller,
        CREATION_OPERATION,
        requested.0,
        fingerprint,
    )
    .await?
    {
        Reservation::Complete(CreationOutcome::Created {
            effective_thread_id,
        }) => service
            .get_session(&caller, &SessionId(effective_thread_id))
            .await
            .map_err(RunFailure::from),
        Reservation::Complete(CreationOutcome::Refused { problem }) => {
            Err(RunFailure::Stored(problem))
        }
        Reservation::Acquired(claim) => {
            let spec = SessionSpec {
                surface,
                environment_profile: environment_profile.map(layer2_core::EnvironmentProfileId),
                geometry: None,
                options: layer2_core::AgentOptions {
                    mode: layer2_core::SessionMode::Conversation,
                    ..Default::default()
                },
                // Profiles and deployment defaults fill `options`; the explicit
                // requirement is what makes an AG-UI-created thread a
                // conversation even on a terminal-defaulting deployment.
                requested_mode: Some(layer2_core::SessionMode::Conversation),
                quoted_rate_card: None,
                caller_credential: credential.map(DelegatedCredential::into_caller_credential),
                runtime_credential: None,
            };
            let task_service = Arc::clone(&service);
            let task_caller = caller.clone();
            let task = tokio::spawn(async move {
                match task_service.create_session(&task_caller, spec).await {
                    Ok(session) => {
                        claim
                            .complete(&CreationOutcome::Created {
                                effective_thread_id: session.id.0.clone(),
                            })
                            .await?;
                        Ok(session)
                    }
                    Err(error) => {
                        let problem = StoredProblem::from_core(error).await?;
                        claim
                            .complete(&CreationOutcome::Refused {
                                problem: problem.clone(),
                            })
                            .await?;
                        Err(RunFailure::Stored(problem))
                    }
                }
            });
            match task.await {
                Ok(result) => result,
                Err(error) => {
                    tracing::error!(%error, "owned AG-UI thread creation task failed");
                    Err(RunFailure::Stored(StoredProblem::creation_task_failed()))
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "thread_alias_tests.rs"]
mod tests;
