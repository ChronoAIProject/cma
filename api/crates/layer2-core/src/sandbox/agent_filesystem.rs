//! Authorized access to the complete mounted Agent home.
//!
//! The control plane never opens a PVC. This service authenticates a caller,
//! resolves the current Agent lineage, and asks the layer-3 runtime adapter to
//! perform a path-confined operation.
use super::{Sandbox, SandboxStatus};
use crate::resource_access::{
    AccessError, AccessPurpose, AuthorizedResource, CallerContext, ResourceAccessError,
    ResourceAccessService, ResourceCapability, ResourceTarget,
};
use crate::AgentId;
use layer3_adapter::{
    AdapterError, AgentFileDeleteReceipt, AgentFileDeleteRequest, AgentFileListReceipt,
    AgentFileListRequest, AgentFileReadReceipt, AgentFileReadRequest, AgentFileWriteReceipt,
    AgentFileWriteRequest, RuntimeHandle, SandboxAdapter,
};
use std::sync::Arc;
use std::time::SystemTime;
#[path = "agent_browser_service.rs"]
mod browser;
#[path = "agent_browser_control_service.rs"]
mod browser_control;
#[path = "agent_environment_service.rs"]
mod environment;

mod read;
mod read_check;
mod scope;
use scope::{public_path, require_home_mutation};
mod write_target;
pub use write_target::FileWriteTarget;

#[derive(Debug, thiserror::Error)]
pub enum AgentFileError {
    #[error(transparent)]
    RuntimeCapacity(super::RuntimeCapacityError),
    #[error(transparent)]
    Access(#[from] ResourceAccessError),
    #[error("invalid file path or request")]
    Invalid,
    #[error("file or directory name exceeds 255 UTF-8 bytes")]
    NameTooLong,
    #[error("file or directory does not exist")]
    NotFound,
    #[error("file exceeds the permitted size")]
    TooLarge,
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
        if let Some(capacity) = super::RuntimeCapacityError::from_adapter(&error) {
            return Self::RuntimeCapacity(capacity);
        }
        match error {
            AdapterError::Unsupported(_) | AdapterError::RuntimeContractMismatch(_) => {
                Self::Unsupported
            }
            AdapterError::Rejected(detail) if detail.starts_with("file_not_found:") => {
                Self::NotFound
            }
            AdapterError::Rejected(detail) if detail.starts_with("file_too_large:") => {
                Self::TooLarge
            }
            AdapterError::Rejected(detail) if detail.contains("not ready") => Self::NotReady,
            AdapterError::Rejected(detail)
                if detail.contains("conflict") || detail.contains("changed") =>
            {
                Self::Conflict
            }
            AdapterError::Rejected(_) | AdapterError::OperationIdentityConflict(_) => Self::Invalid,
            error => {
                use crate::dependency_diagnostics::{Dependency, Failure, Reason};
                let reason = match error {
                    AdapterError::AtCapacity(_) => Reason::Capacity,
                    AdapterError::RuntimeLedgerHold(_) => Reason::RuntimeLedgerHold,
                    AdapterError::InputAuthorityUnavailable(_) => Reason::InputAuthority,
                    AdapterError::PromptNotWritten(_) => Reason::PromptNotWritten,
                    _ => Reason::Unavailable,
                };
                Failure::new(Dependency::Runtime, reason).record();
                Self::Unavailable
            }
        }
    }
}

pub struct AgentFileService {
    access: Arc<ResourceAccessService>,
    adapter: Arc<dyn SandboxAdapter>,
    task_experience: Arc<super::task_experience::TaskExperiencePolicy>,
    environment_cache: Option<Arc<environment::ObservationCache>>,
    read_timings: bool,
}
impl AgentFileService {
    pub fn new(access: Arc<ResourceAccessService>, adapter: Arc<dyn SandboxAdapter>) -> Arc<Self> {
        Arc::new(Self {
            access,
            adapter,
            task_experience: Default::default(),
            environment_cache: None,
            read_timings: false,
        })
    }

    pub fn with_task_experience(
        self: Arc<Self>,
        policy: Arc<super::task_experience::TaskExperiencePolicy>,
    ) -> Arc<Self> {
        Arc::new(Self {
            access: self.access.clone(),
            adapter: self.adapter.clone(),
            task_experience: policy,
            environment_cache: self.environment_cache.clone(),
            read_timings: self.read_timings,
        })
    }

    pub fn with_background_environment(self: Arc<Self>, enabled: bool) -> Arc<Self> {
        Arc::new(Self {
            access: self.access.clone(),
            adapter: self.adapter.clone(),
            task_experience: self.task_experience.clone(),
            environment_cache: enabled.then(|| Arc::new(environment::ObservationCache::new())),
            read_timings: self.read_timings,
        })
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
            agent_id,
            path,
            false,
            |sandbox, boot_id| {
                Ok(AgentFileListRequest {
                    owner_access: authorized
                        .decision
                        .allows(ResourceCapability::ManageSharing),
                    protected_paths: Vec::new(),
                    sandbox_id: sandbox.id.to_string(),
                    runtime_generation: generation(sandbox)?,
                    boot_id,
                    path: path.into(),
                })
            },
        )?;
        let mut response = self
            .adapter
            .list_agent_files(
                &RuntimeHandle(runtime(&authorized.snapshot.lineage.sandbox)?),
                &request,
            )
            .await?;
        self.finish(&authorized, agent_id, ResourceCapability::ViewFiles)
            .await?;
        response
            .entries
            .retain(|entry| public_path(agent_id, &entry.path, false).is_ok());
        Ok(response)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn write(
        &self,
        caller: &crate::Caller,
        context: Option<&CallerContext>,
        credential: Option<&str>,
        agent_id: &AgentId,
        path: &str,
        expected_sha256: Option<String>,
        content_base64: String,
        directory: bool,
        write_target: Option<&FileWriteTarget>,
    ) -> Result<AgentFileWriteReceipt, AgentFileError> {
        let authorized = self
            .authorize(
                caller,
                context,
                credential,
                agent_id,
                ResourceCapability::OperateAgent,
                AccessPurpose::Mutation {
                    started_at: SystemTime::now(),
                },
            )
            .await?;
        if let Some(target) = write_target {
            target.check(self, context, agent_id, &authorized).await?;
        }
        let request = runtime_request(
            &authorized.snapshot.lineage.sandbox,
            agent_id,
            path,
            false,
            |sandbox, boot_id| {
                Ok(AgentFileWriteRequest {
                    owner_access: authorized
                        .decision
                        .allows(ResourceCapability::ManageSharing),
                    protected_paths: Vec::new(),
                    sandbox_id: sandbox.id.to_string(),
                    runtime_generation: generation(sandbox)?,
                    boot_id,
                    path: path.into(),
                    expected_sha256,
                    content_base64,
                    directory,
                })
            },
        )?;
        self.finish(&authorized, agent_id, ResourceCapability::OperateAgent)
            .await?;
        let response = self
            .adapter
            .write_agent_file(
                &RuntimeHandle(runtime(&authorized.snapshot.lineage.sandbox)?),
                &request,
            )
            .await?;
        self.finish(&authorized, agent_id, ResourceCapability::OperateAgent)
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
                ResourceCapability::OperateAgent,
                AccessPurpose::Mutation {
                    started_at: SystemTime::now(),
                },
            )
            .await?;
        let request = runtime_request(
            &authorized.snapshot.lineage.sandbox,
            agent_id,
            path,
            true,
            |sandbox, boot_id| {
                Ok(AgentFileDeleteRequest {
                    owner_access: authorized
                        .decision
                        .allows(ResourceCapability::ManageSharing),
                    protected_paths: Vec::new(),
                    sandbox_id: sandbox.id.to_string(),
                    runtime_generation: generation(sandbox)?,
                    boot_id,
                    path: path.into(),
                    recursive,
                })
            },
        )?;
        self.finish(&authorized, agent_id, ResourceCapability::OperateAgent)
            .await?;
        let response = self
            .adapter
            .delete_agent_file(
                &RuntimeHandle(runtime(&authorized.snapshot.lineage.sandbox)?),
                &request,
            )
            .await?;
        self.finish(&authorized, agent_id, ResourceCapability::OperateAgent)
            .await?;
        Ok(response)
    }

    /// Check the exact runtime operation before selecting an owning manager.
    /// This performs policy reads only; the serving handler still checks before IO.
    pub async fn authorize_runtime_operation(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        id: &AgentId,
        capability: ResourceCapability,
        purpose: AccessPurpose,
        capture_write_target: bool,
    ) -> Result<Option<FileWriteTarget>, AgentFileError> {
        let authorized = self
            .authorize(
                &context.caller,
                Some(context),
                credential,
                id,
                capability,
                purpose,
            )
            .await?;
        let current = self
            .access
            .current_read_snapshot(
                &authorized,
                &ResourceTarget::Agent {
                    agent_id: id.clone(),
                },
                capability,
            )
            .await?;
        Ok(capture_write_target
            .then(|| FileWriteTarget::capture(context, authorized, &current.lineage.sandbox))
            .flatten())
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
        let authorized = if let Some(context) = context {
            if caller != &context.caller || !context.validate_principal() {
                return Err(ResourceAccessError::Access(AccessError::NotFound).into());
            }
            self.access
                .authorize(context, credential, &target, capability, purpose)
                .await?
        } else {
            self.access
                .authorize_personal(caller, &target, capability)
                .await?
        };
        if matches!(purpose, AccessPurpose::Mutation { .. }) {
            // Legacy Agent operation rights never imply consent to modify the
            // complete mounted home. Only an owner or an explicitly accepted
            // view/write link grants that wider filesystem authority.
            require_home_mutation(&authorized, &caller.subject)?;
        }
        Ok(authorized)
    }
    async fn finish(
        &self,
        authorized: &AuthorizedResource,
        id: &AgentId,
        capability: ResourceCapability,
    ) -> Result<(), AgentFileError> {
        let current = self
            .access
            .current_read_snapshot(
                authorized,
                &ResourceTarget::Agent {
                    agent_id: id.clone(),
                },
                capability,
            )
            .await?;
        // current_read_snapshot preserves the original policy fence, including
        // the exact typed link. A surviving legacy OperateAgent grant cannot
        // make an in-flight request adopt authority after that link is revoked.
        if current.lineage.sandbox.runtime != authorized.snapshot.lineage.sandbox.runtime
            || current.lineage.sandbox.lifecycle.home
                != authorized.snapshot.lineage.sandbox.lifecycle.home
            || current.lineage.sandbox.status != SandboxStatus::Active
        {
            return Err(AgentFileError::Conflict);
        }
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
    agent_id: &AgentId,
    path: &str,
    remove: bool,
    build: impl FnOnce(&Sandbox, String) -> Result<T, AgentFileError>,
) -> Result<T, AgentFileError> {
    public_path(agent_id, path, remove)?;
    if sandbox.status != SandboxStatus::Active || sandbox.lifecycle.operation.is_some() {
        return Err(AgentFileError::NotReady);
    }
    let home = sandbox
        .lifecycle
        .home
        .as_ref()
        .ok_or(AgentFileError::Unsupported)?;
    if home.released || home.receipt.is_none() || !home.request.policy.mounts_full_home() {
        return Err(AgentFileError::Unsupported);
    }
    let boot_id = sandbox
        .runtime
        .as_ref()
        .and_then(|r| r.boot_id.clone())
        .ok_or(AgentFileError::NotReady)?;
    build(sandbox, boot_id)
}
