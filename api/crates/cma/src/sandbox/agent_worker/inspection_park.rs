//! Only the bound-runtime read can supply evidence for this disposition.
use super::super::lifecycle_failure::{LifecycleAttempt, LifecycleTaskFailure, Origin};
use super::support::WorkerFailure;
use super::*;
use layer2_core::agent_ownership::OwnershipToken;
use layer2_core::sandbox::{RuntimeBinding, Sandbox};
use layer3_adapter::owner_protocol::InspectionRefusal;

pub(super) struct Inspection {
    claimed: Agent,
    binding: RuntimeBinding,
    refusal: InspectionRefusal,
}

tokio::task_local! {
    pub(super) static ATTEMPT: Option<Arc<OnceLock<Inspection>>>;
}

pub(super) async fn observe<F: Future>(agent: &Agent, sandbox: &Sandbox, read: F) -> F::Output {
    if let (Ok(Some(attempt)), Some(binding)) = (ATTEMPT.try_with(Clone::clone), &sandbox.runtime) {
        let claimed = agent.clone();
        let binding = binding.clone();
        return layer3_adapter::owner_protocol::observe_inspection_failure(
            move |refusal| {
                let _ = attempt.set(Inspection {
                    claimed: claimed.clone(),
                    binding: binding.clone(),
                    refusal,
                });
            },
            read,
        )
        .await;
    }
    read.await
}

pub(super) const INSPECTION_HANDOFF: &str = "agent_inspection_handoff_unconfirmed";

impl AgentWorker {
    pub(super) async fn confirm_inspection(
        &self,
        agent: &Agent,
        sandbox: &Sandbox,
    ) -> Result<(), WorkerFailure> {
        if !self.dependencies.store.agent_inspection_parking_enabled() {
            return Ok(());
        }
        if let Some(binding) = &sandbox.runtime {
            self.dependencies
                .store
                .confirm_agent_inspection(agent, binding)
                .await
                .map_err(|_| {
                    WorkerFailure::retryable(
                        INSPECTION_HANDOFF,
                        "inspection handoff could not be confirmed",
                    )
                })?;
        }
        Ok(())
    }

    pub(super) async fn park_failed_inspection(
        &self,
        attempt: &LifecycleAttempt,
        token: &OwnershipToken,
        observed: &OnceLock<Inspection>,
    ) -> Result<bool, LifecycleTaskFailure> {
        let Some(inspection) = observed.get() else {
            return Ok(false);
        };
        let refusal = &inspection.refusal;
        if refusal.owner.agent_id != token.agent_id.as_str()
            || refusal.owner.instance_id != token.instance_id
            || refusal.owner.epoch != token.epoch
            || refusal.owner.protocol != token.protocol
            || refusal.receiver.runtime_id() != inspection.binding.runtime_id
        {
            return Ok(false);
        }
        self.dependencies
            .store
            .park_agent_inspection(
                token,
                &inspection.claimed,
                &inspection.binding,
                refusal.failure,
            )
            .await
            .map_err(|error| attempt.store(Origin::Execute, error))
    }
}
