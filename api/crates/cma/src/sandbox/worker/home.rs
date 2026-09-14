use std::time::SystemTime;

use layer2_core::rfc3339_at;
use layer2_core::sandbox::{Sandbox, SandboxStoreError};

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
            let next = match crate::sandbox::home::next_home(
                self.dependencies.adapter.as_ref(),
                &sandbox.id,
                sandbox.lifecycle.home.as_deref(),
                self.config.home_policy.as_ref(),
            )
            .await
            {
                Ok(Some(home)) => home,
                Ok(None) => return Ok(None),
                Err(error) => return Ok(Some(adapter_failure(error))),
            };
            let expected = sandbox.revision;
            let mut updated = sandbox.clone();
            updated.lifecycle.home = Some(Box::new(next));
            updated.revision = expected.checked_add(1).ok_or(SandboxStoreError::Conflict)?;
            updated.updated_at = rfc3339_at(now);
            self.dependencies
                .store
                .save_sandbox(expected, updated.clone())
                .await?;
            *sandbox = updated;
        }
    }
}
