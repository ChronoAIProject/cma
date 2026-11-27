use super::{RuntimeBinding, Workspace, WorkspaceWorkStep};
use crate::profiles::content_digest;
use layer3_adapter::workspace_prepare_observation::Observation;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspacePrepareObservation {
    pub observation: Observation,
    /// Zero-based retry count at the time of observation.
    pub attempt: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_generation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub writer_sha256: Option<String>,
}

impl WorkspacePrepareObservation {
    pub fn new(observation: Observation, attempt: u32, binding: Option<&RuntimeBinding>) -> Self {
        Self {
            observation,
            attempt,
            runtime_generation: binding.and_then(|runtime| runtime.provision_generation),
            writer_sha256: binding.and_then(|runtime| content_digest(runtime).ok()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspacePrepareObservations {
    pub operation_id: String,
    pub request_sha256: String,
    /// The first failure committed under the Workspace's work authority.
    pub first: WorkspacePrepareObservation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<WorkspacePrepareObservation>,
}

impl WorkspacePrepareObservations {
    fn record(
        slot: &mut Option<Box<Self>>,
        operation_id: String,
        request_sha256: String,
        sample: WorkspacePrepareObservation,
        terminal: bool,
    ) {
        let observations = slot.get_or_insert_with(|| {
            Box::new(Self {
                operation_id: operation_id.clone(),
                request_sha256: request_sha256.clone(),
                first: sample.clone(),
                terminal: None,
            })
        });
        if terminal
            && observations.operation_id == operation_id
            && observations.request_sha256 == request_sha256
        {
            observations.terminal.get_or_insert(sample);
        }
    }
}

impl Workspace {
    pub fn record_prepare_observation(
        &mut self,
        sample: WorkspacePrepareObservation,
        terminal: bool,
    ) {
        let (slot, operation_id, request_sha256) = match self.work.step {
            WorkspaceWorkStep::Prepare => {
                let Ok(request) = self.original_prepare_request() else {
                    return;
                };
                (
                    &mut self.work.prepare_observations,
                    request.operation_id,
                    request.request_sha256,
                )
            }
            WorkspaceWorkStep::RecoverPrepare => {
                let Some(recovery) = &mut self.work.prepare_recovery else {
                    return;
                };
                let Some(request) = recovery.current_request() else {
                    return;
                };
                let Some(attempt) = recovery.attempts.last_mut() else {
                    return;
                };
                (
                    &mut attempt.observations,
                    request.operation_id,
                    request.request_sha256,
                )
            }
            _ => return,
        };
        WorkspacePrepareObservations::record(slot, operation_id, request_sha256, sample, terminal);
    }
}
