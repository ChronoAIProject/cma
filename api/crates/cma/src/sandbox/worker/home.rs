use std::time::SystemTime;

use layer2_core::rfc3339_at;
use layer2_core::sandbox::{Sandbox, SandboxStoreError};
use layer3_adapter::RuntimeProvisionRequest;

use super::{
    support::{adapter_failure, WorkerFailure},
    SandboxWorker,
};

impl SandboxWorker {
    pub(super) async fn prepare_sandbox_home(
        &self,
        sandbox: &mut Sandbox,
        now: SystemTime,
    ) -> Result<Option<WorkerFailure>, SandboxStoreError> {
        // Existing ephemeral bindings retain their exact request identity. A
        // later revival can allocate a home before restoring their checkpoint.
        if sandbox.runtime.is_some() && sandbox.lifecycle.home.is_none() {
            return Ok(None);
        }
        loop {
            let mut timing = super::provision_timing::ProvisionTiming::new();
            let phase = home_phase(sandbox.lifecycle.home.as_deref());
            let next = match crate::sandbox::home::next_home(
                self.dependencies.adapter.as_ref(),
                &sandbox.id,
                sandbox.lifecycle.home.as_deref(),
                self.config.home_policy.as_ref(),
            )
            .await
            {
                Ok(Some(home)) if sandbox.lifecycle.home.is_none() => {
                    let request = RuntimeProvisionRequest::new(
                        sandbox.lifecycle.runtime_generation_high_water.max(1),
                        crate::sandbox::runtime_mapping::sandbox_spec(
                            sandbox,
                            &self.config.deployment_id,
                        ),
                    );
                    match crate::sandbox::home_native::reserve(
                        self.dependencies.adapter.as_ref(),
                        &request,
                        home,
                    )
                    .await
                    {
                        Ok(home) => home,
                        Err(error) => return Ok(Some(adapter_failure(error))),
                    }
                }
                Ok(Some(home)) => home,
                Ok(None) => {
                    timing.completed(phase);
                    return Ok(None);
                }
                Err(error) => return Ok(Some(adapter_failure(error))),
            };
            timing.completed(phase);
            self.save_home(sandbox, next, now).await?;
            timing.completed("home_persist");
        }
    }

    pub(super) async fn cleanup_native_home(
        &self,
        sandbox: &mut Sandbox,
        now: SystemTime,
    ) -> Result<Option<WorkerFailure>, SandboxStoreError> {
        while let Some(home) = sandbox.lifecycle.home.as_deref() {
            let next = match crate::sandbox::home_native::next_cleanup(
                self.dependencies.adapter.as_ref(),
                home,
            )
            .await
            {
                Ok(Some(next)) => next,
                Ok(None) => return Ok(None),
                Err(error) => return Ok(Some(adapter_failure(error))),
            };
            self.save_home(sandbox, next, now).await?;
        }
        Ok(None)
    }

    async fn save_home(
        &self,
        sandbox: &mut Sandbox,
        home: layer2_core::sandbox::SandboxHome,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        for _ in 0..4 {
            let expected = sandbox.revision;
            let mut updated = sandbox.clone();
            updated.lifecycle.home = Some(Box::new(home.clone()));
            updated.revision = expected.checked_add(1).ok_or(SandboxStoreError::Conflict)?;
            updated.updated_at = updated.updated_at.max(rfc3339_at(now));
            match self
                .dependencies
                .store
                .save_sandbox(expected, updated.clone())
                .await
            {
                Ok(()) => {
                    *sandbox = updated;
                    return Ok(());
                }
                Err(SandboxStoreError::Conflict) => {
                    let current = self
                        .dependencies
                        .store
                        .get_sandbox(&sandbox.id)
                        .await?
                        .ok_or(SandboxStoreError::Conflict)?;
                    // Early Agent admission touches parent activity while this
                    // exact worker is creating its durable native Claim. Keep
                    // that metadata, but never rebase over a different lease,
                    // home, runtime, credential, phase, or deletion intent.
                    let mut previous = current.clone();
                    previous.revision = sandbox.revision;
                    previous.updated_at = sandbox.updated_at.clone();
                    previous.last_activity_at = sandbox.last_activity_at.clone();
                    if previous != *sandbox {
                        return Err(SandboxStoreError::Conflict);
                    }
                    *sandbox = current;
                }
                Err(error) => return Err(error),
            }
        }
        Err(SandboxStoreError::Conflict)
    }
}

fn home_phase(home: Option<&layer2_core::sandbox::SandboxHome>) -> &'static str {
    let Some(home) = home else {
        return "home_reserve";
    };
    if home.receipt.is_some() {
        return "home_verify";
    }
    match home.native_pool.as_ref().map(|native| &native.claim) {
        Some(None) => "home_claim_create",
        Some(Some(claim)) if claim.sandbox_name.is_none() => "home_claim_assignment",
        Some(Some(_)) => "home_adoption",
        None => "home_legacy_prepare",
    }
}
