//! A ledger refusal is pending work, including when ordinary diagnostics are full.
use std::time::SystemTime;

use layer2_core::rfc3339_at;
use layer2_core::sandbox::store::{plan_agent_runtime_ledger_hold_retry, SandboxStoreError};
use layer2_core::sandbox::Agent;

use super::{support::claimed_fence, AgentWorker};

impl AgentWorker {
    pub(super) async fn retry_ledger_hold_at(
        &self,
        agent: Agent,
        now: SystemTime,
        ready_at: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let fence = claimed_fence(&agent, &self.config.holder)?;
        let started = tokio::time::Instant::now();
        let last = self
            .dependencies
            .store
            .agent_log_entry(&agent.id, agent.conversation.last_seq)
            .await?;
        let mutation = plan_agent_runtime_ledger_hold_retry(
            &agent,
            last.as_ref(),
            &self.config.holder,
            fence,
            &rfc3339_at(now + started.elapsed()),
            &rfc3339_at(ready_at + started.elapsed()),
        )?;
        // The store checks the entire log and exact previous marker under the
        // same revision/lease fence. A failed read/commit is never an effect.
        self.dependencies
            .store
            .commit_agent_work(mutation)
            .await
            .map_err(|error| {
                if error == SandboxStoreError::AgentLogCapacityExceeded {
                    // Even the reserved diagnostics can be exhausted/corrupt.
                    // Do not let generic log-capacity settlement fail a live Agent.
                    SandboxStoreError::Unavailable(
                        "runtime ledger hold cannot be recorded; original work remains unchanged"
                            .into(),
                    )
                } else {
                    error
                }
            })
    }
}
