//! Discovery never supplies cleanup authority or caches a proof across yields.
use super::{authority::Authority, SandboxReconciler};
use layer2_core::{
    sandbox::{store::PageRequest, SandboxStoreError},
    SandboxId,
};
use layer3_adapter::recovery_diagnostics::{Observation, Reason, Stage};
use std::time::Duration;
use tokio::time::{timeout_at, Instant};

#[derive(Default)]
pub(super) struct Schedule {
    recovery_turn: bool,
    request: PageRequest,
    pending: Option<SandboxId>,
}

impl Schedule {
    pub(super) fn take_turn(&mut self) -> bool {
        let recovery = self.recovery_turn;
        // Advance before any I/O so cancellation/refusal cannot monopolize work.
        self.recovery_turn = !recovery;
        recovery
    }

    pub(super) async fn run(
        &mut self,
        reconciler: &SandboxReconciler,
    ) -> Result<(), SandboxStoreError> {
        let deadline = Instant::now() + Duration::from_secs(20);
        let service = reconciler
            .failed_runtime_recovery
            .as_ref()
            .expect("configured lane");
        while self.pending.is_none() {
            let mut observation = Observation::new(Stage::Discovery);
            let result = timeout_at(
                deadline.min(Instant::now() + Duration::from_secs(5)),
                reconciler.store.sandbox_runtime_authority(&self.request),
            )
            .await;
            let page = match result {
                Ok(Ok(page)) => page,
                Ok(Err(error)) => {
                    observation.finish(Reason::IncompleteAuthority);
                    return Err(error);
                }
                Err(_) => {
                    observation.finish(Reason::Deadline);
                    return Ok(());
                }
            };
            // Reuse strict, atomic page validation, but discard this local page.
            // Never merge discovery into the ordinary sweep's cleanup authority.
            let mut page_authority = Authority {
                request: self.request.clone(),
                ..Default::default()
            };
            let complete = match page_authority.accept(page) {
                Ok(complete) => complete,
                Err(error) => {
                    observation.finish(Reason::IncompleteAuthority);
                    return Err(error);
                }
            };
            let last = page_authority.items.last_key_value().map(|(id, _)| id);
            for (id, sandbox) in &page_authority.items {
                let latest = service.latest_version_for(sandbox);
                if !sandbox.unpublished_recovery_candidate()
                    || sandbox
                        .automatic_unpublished_recovery_target_for_release(
                            latest.as_deref().unwrap_or_default(),
                        )
                        .is_none()
                {
                    continue;
                }
                if reconciler.binding_holds.contains(id.as_str()) {
                    Observation::new(Stage::Opportunity).skipped(Reason::BindingHeld);
                    continue;
                }
                self.pending = Some(id.clone());
                self.request.cursor = if complete && Some(id) == last {
                    None
                } else {
                    Some(id.to_string())
                };
                break;
            }
            observation.finish(Reason::Success);
            if self.pending.is_some() {
                // Discovery cannot spend a selected candidate's proof budget.
                // Retain only its ID; the next recovery turn starts an empty
                // bounded slice and rereads all authority before any proof.
                return Ok(());
            }
            self.request = page_authority.request;
            if complete {
                return Ok(());
            }
            if Instant::now() >= deadline {
                Observation::new(Stage::Discovery).finish(Reason::IncompleteAuthority);
                return Ok(());
            }
            tokio::task::yield_now().await;
        }
        // Cursor advances before proof/admission. A lost reply is handled by the
        // existing receipt transaction; no native effect or proof is retained here.
        let id = self.pending.take().unwrap();
        let mut observation = Observation::new(Stage::Opportunity);
        if reconciler.binding_holds.contains(id.as_str()) {
            observation.skipped(Reason::BindingHeld);
            return Ok(());
        }
        observation.finish(
            match timeout_at(deadline, service.recover_failed_unpublished(&id)).await {
                Ok(Ok(_)) => Reason::Success,
                Ok(Err(layer2_core::sandbox::SandboxRuntimeUpdateError::Unavailable)) => {
                    Reason::Unavailable
                }
                Ok(Err(_)) => Reason::Ineligible,
                Err(_) => Reason::Deadline,
            },
        );
        Ok(())
    }
}
