use super::*;
use crate::store::native_retirement_action::{
    RetirementAction, RetirementFences, RetirementObservation, RetirementRecords,
};
use layer2_core::sandbox::store::{
    SandboxReconciliationStore, SandboxRuntimeLossOutcome, SandboxRuntimeLossRequest,
    SandboxStoreError,
};

#[derive(Default)]
pub(in super::super) struct FixtureRecords {
    pub observation: Mutex<Option<RetirementObservation>>,
    action: Mutex<Option<RetirementAction>>,
    pub lose_dispatch_acknowledgement: AtomicBool,
    pub store: std::sync::OnceLock<Arc<layer2_core::sandbox::store::InMemorySandboxStore>>,
}

#[async_trait::async_trait]
impl RetirementRecords for FixtureRecords {
    async fn note(
        &self,
        _: &Sandbox,
        observation: &RetirementObservation,
    ) -> Result<(), SandboxStoreError> {
        *self.observation.lock().unwrap() = Some(observation.clone());
        Ok(())
    }

    async fn observation(
        &self,
        _: &SandboxId,
    ) -> Result<Option<(RetirementObservation, RetirementFences)>, SandboxStoreError> {
        Ok(self
            .observation
            .lock()
            .unwrap()
            .clone()
            .map(|observation| (observation, [None; 2])))
    }

    async fn action(&self, _: &SandboxId) -> Result<Option<RetirementAction>, SandboxStoreError> {
        Ok(self.action.lock().unwrap().clone())
    }

    async fn admit(&self, _: &Sandbox, action: &RetirementAction) -> Result<(), SandboxStoreError> {
        let mut current = self.action.lock().unwrap();
        if current.is_some() {
            return Err(SandboxStoreError::Conflict);
        }
        *current = Some(action.clone());
        Ok(())
    }

    async fn transition(
        &self,
        _: &Sandbox,
        previous: &RetirementAction,
        next: Option<&RetirementAction>,
    ) -> Result<(), SandboxStoreError> {
        let mut current = self.action.lock().unwrap();
        if current.as_ref() != Some(previous) {
            return Err(SandboxStoreError::Conflict);
        }
        *current = next.cloned();
        if next.is_some()
            && self
                .lose_dispatch_acknowledgement
                .swap(false, Ordering::SeqCst)
        {
            return Err(SandboxStoreError::Unavailable(
                "synthetic lost dispatch acknowledgement".into(),
            ));
        }
        Ok(())
    }

    async fn settle(
        &self,
        request: SandboxRuntimeLossRequest,
        action: &RetirementAction,
    ) -> Result<SandboxRuntimeLossOutcome, SandboxStoreError> {
        if self.action.lock().unwrap().as_ref() != Some(action) {
            return Err(SandboxStoreError::Conflict);
        }
        let outcome = self
            .store
            .get()
            .unwrap()
            .recover_runtime_loss(request)
            .await?;
        if outcome == SandboxRuntimeLossOutcome::RecoveryScheduled {
            *self.action.lock().unwrap() = None;
        }
        Ok(outcome)
    }
}

pub(in super::super) fn pod() -> layer3_adapter::RetirementPod {
    layer3_adapter::RetirementPod {
        uid: "fixture-original-pod".into(),
        container_id: "fixture-runtime-container".into(),
        restart_count: 0,
    }
}

use crate::store::native_retirement_action::capture::{
    CapturedRetirementObservation, RetirementCaptureConsumer,
};

#[derive(Default)]
pub(super) struct UnavailableCapture(pub(super) AtomicUsize);

#[async_trait::async_trait]
impl RetirementCaptureConsumer for UnavailableCapture {
    async fn offer(&self, _: &Sandbox, _: &Agent) -> Result<(), SandboxStoreError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(SandboxStoreError::Unavailable(
            "capture store unavailable".into(),
        ))
    }

    async fn observed(
        &self,
        _: &Sandbox,
        _: &[Agent],
    ) -> Result<Option<CapturedRetirementObservation>, SandboxStoreError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(SandboxStoreError::Unavailable(
            "capture store unavailable".into(),
        ))
    }

    async fn admit(
        &self,
        _: &Sandbox,
        _: &RetirementAction,
        _: &CapturedRetirementObservation,
    ) -> Result<(), SandboxStoreError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(SandboxStoreError::Unavailable(
            "capture store unavailable".into(),
        ))
    }
}
