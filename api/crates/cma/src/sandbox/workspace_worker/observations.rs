use super::*;
use layer2_core::sandbox::store::{ClaimedWork, WorkAuthority};
use layer2_core::sandbox::WorkspacePrepareObservation;
pub(super) use layer3_adapter::workspace_prepare_observation::Stage;
use layer3_adapter::workspace_prepare_observation::{observe, Code, Observation};

pub(super) const INSPECTION_FAILURE: Observation = Observation {
    stage: Stage::RuntimeInspection,
    code: Code::Local,
    wire_retryable: None,
};

pub(super) const INVALID_RECEIPT: Observation = Observation {
    stage: Stage::ReceiptValidation,
    code: Code::InvalidReceipt,
    wire_retryable: None,
};

impl WorkerFailure {
    pub(super) fn invalid_prepare_receipt() -> Self {
        Self::invalid_receipt().observed(INVALID_RECEIPT)
    }

    pub(super) fn observed(mut self, observation: Observation) -> Self {
        self.observation = Some(observation);
        self
    }

    pub(super) fn observed_rpc(self, observation: Option<Observation>, stage: Stage) -> Self {
        self.observed(observation.unwrap_or(Observation {
            stage,
            code: Code::Unknown,
            wire_retryable: None,
        }))
    }
}

impl WorkspaceWorker {
    pub(super) async fn observe_prepare_rpc<T>(
        &self,
        call: impl Future<Output = T>,
    ) -> (T, Option<Observation>) {
        if self.config.prepare_observations {
            observe(call).await
        } else {
            (call.await, None)
        }
    }

    pub(super) fn record_prepare_failure(
        &self,
        workspace: &mut Workspace,
        failure: &WorkerFailure,
    ) {
        let Some(observation) = failure
            .observation
            .filter(|_| self.config.prepare_observations)
        else {
            return;
        };
        // The settlement transaction fences both this claim and its captured parent.
        let Ok(authorities) = WorkAuthority::current() else {
            return;
        };
        let Some(parent) = authorities
            .iter()
            .find_map(|authority| match authority.claimed() {
                ClaimedWork::Workspace(row) if row.id == workspace.id => authority.parent(),
                _ => None,
            })
        else {
            return;
        };
        let sample = WorkspacePrepareObservation::new(
            observation,
            workspace.work.attempts,
            parent.runtime.as_ref(),
        );
        workspace.record_prepare_observation(sample, !failure.retryable);
    }
}
