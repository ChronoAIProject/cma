use super::*;
use crate::bridge::invoke::{BridgeInvokeEffect, BridgeInvokeRequest};

impl ChannelControlExecutionService {
    /// Readiness permits replacing an expired, undispatched claim. It grants
    /// no execution authorization; dispatch must obtain its own exact live claim.
    pub async fn ready_for_claim(&self, command: &AdmittedAgentCommand) -> Result<(), Error> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        let work = async {
            command.validate().map_err(|_| Error::Corrupt)?;
            let snapshot = self.store.read_control_execution(command, deadline).await?;
            if snapshot.admitted.command != *command {
                return Err(Error::Conflict);
            }
            let binding = snapshot.admitted.sidecar.intent.reference.binding();
            let proof = self
                .invokes
                .authorize(BridgeInvokeRequest {
                    grant_id: binding.consent.view.grant_id.clone(),
                    pin: binding.source.pin().clone(),
                    target: binding.source.target().clone(),
                    effect: BridgeInvokeEffect::ExecuteCommand {
                        command_id: binding.source.command_id().clone(),
                    },
                })
                .await
                .map_err(invoke_error)?;
            validation::require_ready_snapshot(&proof, &snapshot)?;
            proof.require_current().map_err(invoke_error)?;
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::Unavailable);
            }
            Ok(())
        };
        tokio::time::timeout_at(deadline, work)
            .await
            .map_err(|_| Error::Unavailable)?
    }
}
