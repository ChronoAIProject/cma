use super::*;
use layer2_core::sandbox::store::{ClaimedWork, WorkAuthority, WORK_LEASE_SECONDS};

impl SandboxWorker {
    pub(super) async fn run_renewable(
        &self,
        sandbox: Sandbox,
        now: SystemTime,
        claiming: tokio::time::Instant,
    ) -> Result<bool, SandboxStoreError> {
        let authority =
            WorkAuthority::new(ClaimedWork::Sandbox(Box::new(sandbox.clone())), claiming);
        let claim_ms = claiming.elapsed().as_millis() as u64;
        // Keep the authority scope's stack frame bounded: worker execution is
        // a large state machine and must not be embedded in the renewal loop.
        let work = Box::pin(layer2_core::agent_ownership::transport_observation_scope(
            self.run_claimed(sandbox.clone(), now, claim_ms),
        ));
        let result = self
            .dependencies
            .store
            .scope_sandbox_publication(Box::pin(async {
                authority
                    .run(work, || {
                        Box::pin(async {
                            let at = now + claiming.elapsed();
                            let grant = self
                                .dependencies
                                .store
                                .renew_sandbox_authority(
                                    &sandbox,
                                    &rfc3339_at(at),
                                    &rfc3339_at(at + Duration::from_secs(WORK_LEASE_SECONDS)),
                                )
                                .await?;
                            if let Some(grant) = &grant {
                                tracing::debug!(sandbox_id = %sandbox.id,
                                    valid_for_ms = grant.valid_for.as_millis() as u64,
                                    "Sandbox work authority renewed");
                            }
                            Ok(grant)
                        })
                    })
                    .await
            }))
            .await;
        if result.is_err() {
            tracing::warn!(sandbox_id = %sandbox.id, step = ?sandbox.work.step,
                "Sandbox work ended without renewed publication authority");
        }
        result
    }
}
