//! Explicit recovery of an unpublished, indeterminate Workspace preparation.
//! Original admission and retained files remain immutable evidence.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    AdapterError, Result, RuntimeStatus, RuntimeStatusRequest, WorkspaceReceipt, WorkspaceRequest,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspacePrepareRecoveryRequest {
    pub operation_id: String,
    pub request_sha256: String,
    pub original: WorkspaceRequest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_recovery_operation_id: Option<String>,
}

impl WorkspacePrepareRecoveryRequest {
    pub fn new(
        operation_id: impl Into<String>,
        original: WorkspaceRequest,
        previous_recovery_operation_id: Option<String>,
    ) -> Self {
        let mut request = Self {
            operation_id: operation_id.into(),
            request_sha256: String::new(),
            original,
            previous_recovery_operation_id,
        };
        request.request_sha256 = request.computed_sha256();
        request
    }

    pub fn computed_sha256(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(b"chrono.workspace.prepare-recovery.v1\0");
        // Canonical tuple serialization includes the original operation and its
        // digest as well as every source field and the immediately prior attempt.
        digest.update(
            serde_json::to_vec(&(
                &self.operation_id,
                &self.original,
                &self.previous_recovery_operation_id,
            ))
            .expect("recovery fields serialize"),
        );
        format!("{:x}", digest.finalize())
    }

    pub fn has_valid_sha256(&self) -> bool {
        self.request_sha256 == self.computed_sha256() && self.original.has_valid_sha256()
    }

    pub fn preserved_path(&self) -> String {
        format!(
            "/home/cma/.cma/prepare-recovery/{}/original",
            self.original.workspace_id
        )
    }

    pub fn validate(&self) -> Result<()> {
        let valid_id = |value: &str| {
            !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
        };
        if !self.has_valid_sha256()
            || !valid_id(&self.operation_id)
            || self.operation_id == self.original.operation_id
            || self.original.operation_id
                != format!("workspace:{}:prepare:v1", self.original.workspace_id)
            || self.original.workspace_id.is_empty()
            || self.original.workspace_id.len() > 128
            || !self
                .original
                .workspace_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            || self
                .previous_recovery_operation_id
                .as_deref()
                .is_some_and(|previous| {
                    !valid_id(previous)
                        || previous == self.operation_id
                        || previous == self.original.operation_id
                })
        {
            return Err(invalid());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspacePrepareRecoveryReceipt {
    pub prepared: WorkspaceReceipt,
    pub preserved_path: String,
    pub original_journal_sha256: String,
}

/// Current boot authority is transport evidence, not part of the replay digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspacePrepareRecoveryFrame {
    pub runtime: RuntimeStatus,
    pub request: WorkspacePrepareRecoveryRequest,
}

impl WorkspacePrepareRecoveryFrame {
    pub fn runtime_request(&self) -> RuntimeStatusRequest {
        RuntimeStatusRequest {
            sandbox_id: self.runtime.sandbox_id.clone(),
            generation: self.runtime.generation,
            home_identity: self.runtime.home_identity.clone(),
        }
    }

    pub fn validate(&self) -> Result<()> {
        self.request.validate()?;
        self.runtime.validate_for(&self.runtime_request())?;
        if !self.runtime.installation_ready
            || !self.runtime.credential_ready
            || self.runtime.restore_pending
        {
            return Err(invalid());
        }
        Ok(())
    }
}

fn invalid() -> AdapterError {
    AdapterError::Rejected("Workspace preparation recovery requires exact original admission, ordered retry and current credentialed runtime authority".into())
}
