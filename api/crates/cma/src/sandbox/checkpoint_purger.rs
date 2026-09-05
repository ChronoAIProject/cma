//! Sandbox-scoped checkpoint archive cleanup for terminal retirement.

use std::sync::Arc;

use layer2_core::sandbox::Sandbox;
use layer3_adapter::AdapterError;

use super::worker::{CheckpointObjectPurger, SandboxDependencyError};
use crate::store::MongoCheckpointObjectStore;

pub(crate) struct DurableCheckpointObjectPurger {
    objects: Arc<MongoCheckpointObjectStore>,
}

impl DurableCheckpointObjectPurger {
    pub(crate) fn new(objects: Arc<MongoCheckpointObjectStore>) -> Self {
        Self { objects }
    }
}

#[async_trait::async_trait]
impl CheckpointObjectPurger for DurableCheckpointObjectPurger {
    async fn purge_checkpoint_objects(
        &self,
        sandbox: &Sandbox,
        _operation_id: &str,
    ) -> Result<(), SandboxDependencyError> {
        self.objects
            .purge_sandbox(sandbox.id.as_str())
            .await
            .map_err(|error| classify(&sandbox.id.to_string(), error))
    }
}

fn classify(sandbox_id: &str, error: AdapterError) -> SandboxDependencyError {
    tracing::error!(
        sandbox_id,
        error = %error,
        "checkpoint archive purge failed"
    );
    match error {
        AdapterError::Unavailable(_) | AdapterError::AtCapacity(_) => {
            SandboxDependencyError::Transient("checkpoint archive storage is unavailable".into())
        }
        AdapterError::Rejected(_) | AdapterError::RuntimeContractMismatch(_) => {
            SandboxDependencyError::Security(
                "checkpoint archive authority refused the sandbox boundary".into(),
            )
        }
        AdapterError::Unsupported(_) => {
            SandboxDependencyError::Validation("checkpoint archive cleanup is not supported".into())
        }
    }
}
