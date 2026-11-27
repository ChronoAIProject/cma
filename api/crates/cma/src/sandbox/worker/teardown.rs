//! Runtime finality precedes funding settlement and terminal retirement.
use super::{
    support::{adapter_failure, funding_failure},
    SandboxWorker,
};
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{Sandbox, SandboxEventKind, SandboxWorkStep};
use layer3_adapter::RuntimeHandle;
use std::time::SystemTime;

use super::teardown_timing::{adapter_outcome, funding_outcome, Phase, TeardownTiming};

impl SandboxWorker {
    pub(super) async fn teardown(
        &self,
        mut sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        if let Some(failure) = self.prepare_retirement_cleanup(&mut sandbox, now).await? {
            self.suspend_credential_blocked_retirement(&mut sandbox, &failure)
                .await?;
            return self.retry_failure(sandbox, failure, now).await;
        }
        let funding_completion = sandbox.clone();
        let timing = TeardownTiming::new(&sandbox.id, sandbox.work.attempts);
        // Suspension may have reached Kubernetes before its database acknowledgement.
        // Native deletion proves CRD and Pod absence without a running process.
        let retained = sandbox.lifecycle.suspended_runtime.as_deref().or_else(|| {
            sandbox.runtime.as_ref().filter(|binding| {
                sandbox.lifecycle.home.is_some() && binding.runtime_id.starts_with("agent-sandbox:")
            })
        });
        if let Some(binding) = retained {
            if let Some(home) = &sandbox.lifecycle.home {
                if let Err(error) = timing
                    .observe(
                        Phase::RetainedHomeRoute,
                        async {
                            self.dependencies.adapter.verify_retained_home_route(
                                &RuntimeHandle::new(&binding.runtime_id),
                                &home.request,
                            )
                        },
                        adapter_outcome,
                    )
                    .await
                {
                    return self
                        .retry_failure(sandbox, adapter_failure(error), now)
                        .await;
                }
            }
            if let Err(error) = timing
                .observe(
                    Phase::RetainedRuntime,
                    self.dependencies
                        .adapter
                        .teardown_retained_runtime(&RuntimeHandle::new(&binding.runtime_id)),
                    adapter_outcome,
                )
                .await
            {
                return self
                    .retry_failure(sandbox, adapter_failure(error), now)
                    .await;
            }
            sandbox.lifecycle.suspended_runtime = None;
            sandbox.runtime = None;
        }
        if let Some(binding) = &sandbox.runtime {
            if let Err(error) = timing
                .observe(
                    Phase::AdoptRuntime,
                    crate::sandbox::runtime_attachment::adopt_bound_runtime(
                        self.dependencies.adapter.as_ref(),
                        &sandbox,
                        &self.config.deployment_id,
                    ),
                    adapter_outcome,
                )
                .await
            {
                let absent = timing
                    .observe(
                        Phase::RuntimeInventory,
                        self.dependencies.adapter.runtime_inventory(),
                        adapter_outcome,
                    )
                    .await
                    .is_ok_and(|inventory| {
                        inventory
                            .iter()
                            .all(|item| item.handle.0 != binding.runtime_id)
                    });
                if !absent {
                    return self
                        .retry_failure(sandbox, adapter_failure(error), now)
                        .await;
                }
                sandbox.runtime = None;
            }
        }
        if let Some(binding) = &sandbox.runtime {
            if let Err(error) = timing
                .observe(
                    Phase::Runtime,
                    self.dependencies
                        .adapter
                        .teardown(&RuntimeHandle(binding.runtime_id.clone())),
                    adapter_outcome,
                )
                .await
            {
                return self
                    .retry_failure(sandbox, adapter_failure(error), now)
                    .await;
            }
        }
        if let Err(error) = timing
            .observe(
                Phase::Funding,
                self.funding.settle(&funding_completion, now),
                funding_outcome,
            )
            .await
        {
            return self
                .retry_failure(sandbox, funding_failure(error), now)
                .await;
        }
        sandbox.runtime = None;
        timing
            .observe(
                Phase::Publication,
                self.advance(
                    sandbox,
                    SandboxWorkStep::MarkRetired,
                    now,
                    SandboxEventKind::WorkStepCompleted {
                        step: SandboxWorkStep::TeardownRuntime,
                    },
                ),
                |_| "store_error",
            )
            .await
    }
}
