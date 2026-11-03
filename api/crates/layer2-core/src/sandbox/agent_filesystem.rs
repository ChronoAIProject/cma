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

mod read_check;

#[derive(Debug, thiserror::Error)]
pub enum AgentFileError {
    #[error(transparent)]
    Access(#[from] ResourceAccessError),
    #[error("invalid file path or request")]
    Invalid,
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

    pub async fn environment(
        &self,
        caller: &crate::Caller,
        context: Option<&CallerContext>,
        credential: Option<&str>,
        agent_id: &AgentId,
    ) -> Result<super::agent_environment::AgentEnvironment, AgentFileError> {
        use super::agent_environment::{
            AgentEnvironment, EnvironmentAvailability as State, EnvironmentFeature,
        };
        let authorized = self
            .authorize(
                caller,
                context,
                credential,
                agent_id,
                ResourceCapability::ViewDetails,
                AccessPurpose::Read,
            )
            .await?;
        let sandbox = &authorized.snapshot.lineage.sandbox;
        let (state, reason) = if !authorized.decision.allows(ResourceCapability::ViewFiles) {
            (State::Forbidden, "view_files_required")
        } else if sandbox.status != SandboxStatus::Active {
            (State::NotEnabled, "runtime_not_active")
        } else {
            match runtime_request(sandbox, agent_id, "", false, |sandbox, _| {
                generation(sandbox)
            }) {
                Ok(_) => (State::Available, "authorized_mounted_home"),
                Err(AgentFileError::Unsupported) => {
                    (State::Unsupported, "mounted_home_unsupported")
                }
                Err(_) => (State::TemporarilyUnavailable, "runtime_not_ready"),
            }
        };
        let writable = matches!(state, State::Available)
            && require_home_mutation(&authorized, &caller.subject).is_ok();
        let history = authorized.decision.allows(ResourceCapability::ViewHistory);
        Ok(AgentEnvironment {
            agent_id: agent_id.to_string(),
            policy_revision: authorized.snapshot.fence().policy_revision,
            runtime_incarnation: AgentEnvironment::incarnation(agent_id, sandbox),
            revalidate_at: authorized.decision.revalidate_at.map(crate::rfc3339_at),
            files: EnvironmentFeature {
                state,
                reason,
                scope: "mounted_home",
                writable,
            },
            operation_output: EnvironmentFeature::history(history, "recorded_output_only"),
            browser: EnvironmentFeature::unsupported(),
            replay: EnvironmentFeature::history(history, "operation_records_only"),
            control: EnvironmentFeature::unsupported(),
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
            agent_id,
            path,
            false,
            |sandbox, boot_id| {
                Ok(AgentFileReadRequest {
                    owner_access: authorized
                        .decision
                        .allows(ResourceCapability::ManageSharing),
                    protected_paths: Vec::new(),
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

fn require_home_mutation(
    authorized: &AuthorizedResource,
    subject: &crate::SubjectId,
) -> Result<(), AgentFileError> {
    let owner = authorized
        .decision
        .allows(ResourceCapability::ManageSharing);
    let linked_write = authorized.snapshot.shares.iter().any(|share| {
        share.mode == crate::resource_access::ShareMode::ViewWrite
            && share.revoked_at.is_none()
            && authorized
                .decision
                .share_revisions
                .contains(&(share.id.clone(), share.revision))
            && share
                .link
                .as_ref()
                .is_some_and(|link| link.accepted_subjects.contains(subject))
    });
    if owner || linked_write {
        Ok(())
    } else {
        Err(ResourceAccessError::Access(AccessError::Forbidden).into())
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

fn public_path(agent_id: &AgentId, path: &str, remove: bool) -> Result<(), AgentFileError> {
    if !layer3_adapter::valid_agent_file_path(path)
        || [".cma", "cma/.cma"].iter().any(|protected| {
            path == *protected
                || path.starts_with(&format!("{protected}/"))
                || (remove && (path.is_empty() || protected.starts_with(&format!("{path}/"))))
        })
    {
        return Err(AgentFileError::Invalid);
    }
    if (remove && path == "attachments")
        || path
            .strip_prefix("attachments/")
            .is_some_and(|suffix| suffix.split('/').next() != Some(agent_id.0.as_str()))
    {
        return Err(AgentFileError::Invalid);
    }
    Ok(())
}
