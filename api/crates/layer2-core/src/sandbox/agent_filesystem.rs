//! Authorized access to the complete mounted Agent home.
//!
//! The control plane never opens a PVC. This service authenticates a caller,
//! resolves the current Agent lineage, and asks the layer-3 runtime adapter to
//! perform a path-confined operation.
use super::{Sandbox, SandboxStatus};
use crate::resource_access::{
    AccessError, AccessPurpose, AuthorizedResource, CallerContext, ResourceAccessError,
    ResourceAccessService, ResourceActor, ResourceCapability, ResourceTarget,
};
use crate::AgentId;
use layer3_adapter::{
    AdapterError, AgentFileDeleteReceipt, AgentFileDeleteRequest, AgentFileListReceipt,
    AgentFileListRequest, AgentFileReadReceipt, AgentFileReadRequest, AgentFileWriteReceipt,
    AgentFileWriteRequest, RuntimeHandle, SandboxAdapter,
};
use std::sync::Arc;
use std::time::SystemTime;

#[derive(Debug, thiserror::Error)]
pub enum AgentFileError {
    #[error(transparent)]
    Access(#[from] ResourceAccessError),
    #[error("invalid file path or request")]
    Invalid,
    #[error("the Agent runtime is not available")]
    NotReady,
    #[error("the current file changed; refresh before saving")]
    Conflict,
    #[error("filesystem access is unavailable on this runtime")]
    Unsupported,
    #[error("filesystem operation is temporarily unavailable")]
    Unavailable,
}
impl From<AdapterError> for AgentFileError {
    fn from(error: AdapterError) -> Self {
        match error {
            AdapterError::Unsupported(_) | AdapterError::RuntimeContractMismatch(_) => {
                Self::Unsupported
            }
            AdapterError::Rejected(detail) if detail.contains("not ready") => Self::NotReady,
            AdapterError::Rejected(detail)
                if detail.contains("conflict") || detail.contains("changed") =>
            {
                Self::Conflict
            }
            AdapterError::Rejected(_) | AdapterError::OperationIdentityConflict(_) => Self::Invalid,
            _ => Self::Unavailable,
        }
    }
}

pub struct AgentFileService {
    access: Arc<ResourceAccessService>,
    adapter: Arc<dyn SandboxAdapter>,
}
impl AgentFileService {
    pub fn new(access: Arc<ResourceAccessService>, adapter: Arc<dyn SandboxAdapter>) -> Arc<Self> {
        Arc::new(Self { access, adapter })
    }

    pub async fn list(
        &self,
        caller: &crate::Caller,
        context: Option<&CallerContext>,
        credential: Option<&str>,
        agent_id: &AgentId,
        path: &str,
    ) -> Result<AgentFileListReceipt, AgentFileError> {
        let authorized = self
            .authorize(
                caller,
                context,
                credential,
                agent_id,
                ResourceCapability::ViewFiles,
                AccessPurpose::Read,
            )
            .await?;
        let request = runtime_request(
            &authorized.snapshot.lineage.sandbox,
            path,
            |sandbox, boot_id| {
                Ok(AgentFileListRequest {
                    sandbox_id: sandbox.id.to_string(),
                    runtime_generation: generation(sandbox)?,
                    boot_id,
                    path: path.into(),
                })
            },
        )?;
        let response = self
            .adapter
            .list_agent_files(
                &RuntimeHandle(runtime(&authorized.snapshot.lineage.sandbox)?),
                &request,
            )
            .await?;
        self.finish(&authorized, agent_id, ResourceCapability::ViewFiles)
            .await?;
        Ok(response)
    }

    pub async fn read(
        &self,
        caller: &crate::Caller,
        context: Option<&CallerContext>,
        credential: Option<&str>,
        agent_id: &AgentId,
        path: &str,
        max_bytes: u64,
    ) -> Result<AgentFileReadReceipt, AgentFileError> {
        let authorized = self
            .authorize(
                caller,
                context,
                credential,
                agent_id,
                ResourceCapability::ViewFiles,
                AccessPurpose::Read,
            )
            .await?;
        let request = runtime_request(
            &authorized.snapshot.lineage.sandbox,
            path,
            |sandbox, boot_id| {
                Ok(AgentFileReadRequest {
                    sandbox_id: sandbox.id.to_string(),
                    runtime_generation: generation(sandbox)?,
                    boot_id,
                    path: path.into(),
                    max_bytes,
                })
            },
        )?;
        let response = self
            .adapter
            .read_agent_file(
                &RuntimeHandle(runtime(&authorized.snapshot.lineage.sandbox)?),
                &request,
            )
            .await?;
        self.finish(&authorized, agent_id, ResourceCapability::ViewFiles)
            .await?;
        Ok(response)
    }

    pub async fn write(
        &self,
        caller: &crate::Caller,
        context: Option<&CallerContext>,
        credential: Option<&str>,
        agent_id: &AgentId,
        path: &str,
        expected_sha256: Option<String>,
        content_base64: String,
    ) -> Result<AgentFileWriteReceipt, AgentFileError> {
        let authorized = self
            .authorize(
                caller,
                context,
                credential,
                agent_id,
                ResourceCapability::ManageSharing,
                AccessPurpose::Mutation {
                    started_at: SystemTime::now(),
                },
            )
            .await?;
        let request = runtime_request(
            &authorized.snapshot.lineage.sandbox,
            path,
            |sandbox, boot_id| {
                Ok(AgentFileWriteRequest {
                    sandbox_id: sandbox.id.to_string(),
                    runtime_generation: generation(sandbox)?,
                    boot_id,
                    path: path.into(),
                    expected_sha256,
                    content_base64,
                })
            },
        )?;
        let response = self
            .adapter
            .write_agent_file(
                &RuntimeHandle(runtime(&authorized.snapshot.lineage.sandbox)?),
                &request,
            )
            .await?;
        self.finish(&authorized, agent_id, ResourceCapability::ManageSharing)
            .await?;
        Ok(response)
    }

    pub async fn delete(
        &self,
        caller: &crate::Caller,
        context: Option<&CallerContext>,
        credential: Option<&str>,
        agent_id: &AgentId,
        path: &str,
        recursive: bool,
    ) -> Result<AgentFileDeleteReceipt, AgentFileError> {
        let authorized = self
            .authorize(
                caller,
                context,
                credential,
                agent_id,
                ResourceCapability::ManageSharing,
                AccessPurpose::Mutation {
                    started_at: SystemTime::now(),
                },
            )
            .await?;
        let request = runtime_request(
            &authorized.snapshot.lineage.sandbox,
            path,
            |sandbox, boot_id| {
                Ok(AgentFileDeleteRequest {
                    sandbox_id: sandbox.id.to_string(),
                    runtime_generation: generation(sandbox)?,
                    boot_id,
                    path: path.into(),
                    recursive,
                })
            },
        )?;
        let response = self
            .adapter
            .delete_agent_file(
                &RuntimeHandle(runtime(&authorized.snapshot.lineage.sandbox)?),
                &request,
            )
            .await?;
        self.finish(&authorized, agent_id, ResourceCapability::ManageSharing)
            .await?;
        Ok(response)
    }

    async fn authorize(
        &self,
        caller: &crate::Caller,
        context: Option<&CallerContext>,
        credential: Option<&str>,
        id: &AgentId,
        capability: ResourceCapability,
        purpose: AccessPurpose,
    ) -> Result<AuthorizedResource, AgentFileError> {
        let target = ResourceTarget::Agent {
            agent_id: id.clone(),
        };
        if let Some(context) = context {
            if caller != &context.caller
                || !context.validate_principal()
                || !matches!(&context.actor, ResourceActor::Human { subject } | ResourceActor::Api { subject, delegation_id: None } if subject == &caller.subject)
            {
                return Err(ResourceAccessError::Access(AccessError::NotFound).into());
            }
            Ok(self
                .access
                .authorize(context, credential, &target, capability, purpose)
                .await?)
        } else {
            Ok(self
                .access
                .authorize_personal(caller, &target, capability)
                .await?)
        }
    }
    async fn finish(
        &self,
        authorized: &AuthorizedResource,
        id: &AgentId,
        capability: ResourceCapability,
    ) -> Result<(), AgentFileError> {
        self.access
            .current_read_snapshot(
                authorized,
                &ResourceTarget::Agent {
                    agent_id: id.clone(),
                },
                capability,
            )
            .await?;
        Ok(())
    }
}

fn runtime(sandbox: &Sandbox) -> Result<String, AgentFileError> {
    sandbox
        .runtime
        .as_ref()
        .map(|runtime| runtime.runtime_id.clone())
        .ok_or(AgentFileError::NotReady)
}
fn generation(sandbox: &Sandbox) -> Result<u64, AgentFileError> {
    sandbox
        .runtime
        .as_ref()
        .and_then(|r| r.provision_generation)
        .ok_or(AgentFileError::NotReady)
}
fn runtime_request<T>(
    sandbox: &Sandbox,
    path: &str,
    build: impl FnOnce(&Sandbox, String) -> Result<T, AgentFileError>,
) -> Result<T, AgentFileError> {
    if sandbox.status != SandboxStatus::Active || !layer3_adapter::valid_agent_file_path(path) {
        return Err(AgentFileError::NotReady);
    }
    let boot_id = sandbox
        .runtime
        .as_ref()
        .and_then(|r| r.boot_id.clone())
        .ok_or(AgentFileError::NotReady)?;
    build(sandbox, boot_id)
}
