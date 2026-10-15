//! Owner-selected files uploaded into the Agent's retained environment.
use super::{AgentStatus, Sandbox, SandboxStatus, WorkspaceStatus, WorktreeStatus};
use crate::{
    resource_access::{
        AccessError, AccessPurpose, AuthorizedResource, CallerContext, ResourceAccessError,
        ResourceAccessService, ResourceAccessSnapshot, ResourceAccessStoreError, ResourceActor,
        ResourceCapability, ResourceTarget,
    },
    AgentId, Caller,
};
use layer3_adapter::{
    AdapterError, AgentAttachmentRequest, AgentSlot, RuntimeHandle, RuntimeStatusRequest,
    SandboxAdapter,
};
use sha2::{Digest as _, Sha256};
use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

pub const MAX_ATTACHMENT_BYTES: usize = 20 * 1024 * 1024;
pub const ATTACHMENT_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, thiserror::Error)]
pub enum AttachmentError {
    #[error(transparent)]
    Access(#[from] ResourceAccessError),
    #[error("attachment name, media type, or operation key is invalid")]
    Invalid,
    #[error("attachment exceeds 20 MiB")]
    TooLarge,
    #[error("the Agent environment is not available")]
    NotReady,
    #[error("this retained runtime does not support attachment upload")]
    Unsupported,
    #[error("the upload identity or environment changed")]
    Conflict,
    #[error("attachment upload is temporarily unavailable")]
    Unavailable,
}
impl From<AdapterError> for AttachmentError {
    fn from(error: AdapterError) -> Self {
        match error {
            AdapterError::Unsupported(_) | AdapterError::RuntimeContractMismatch(_) => {
                Self::Unsupported
            }
            AdapterError::OperationIdentityConflict(_) | AdapterError::Rejected(_) => {
                Self::Conflict
            }
            _ => Self::Unavailable,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentAttachment {
    pub id: String,
    pub agent_id: AgentId,
    pub name: String,
    pub path: String,
    pub size_bytes: u64,
    pub media_type: String,
    pub sha256: String,
}

/// Opaque authorization retained across bounded body reception, revalidated before IO.
pub struct AttachmentAdmission {
    authorized: AuthorizedResource,
    target: ResourceTarget,
    identity: Vec<u8>,
    id: String,
    name: String,
    media_type: String,
    deadline: tokio::time::Instant,
}

pub struct AttachmentMetadata<'a> {
    pub key: &'a str,
    pub name: &'a str,
    pub media_type: &'a str,
}

/// Persist one complete file in the exact selected environment. Implementations
/// must prove storage/runtime identity and return only an atomic publication.
#[async_trait::async_trait]
pub trait AttachmentWriter: Send + Sync {
    async fn write(
        &self,
        sandbox: &Sandbox,
        attachment: &AgentAttachment,
        content: Vec<u8>,
    ) -> Result<AgentAttachment, AttachmentError>;
}
struct RuntimeAttachmentWriter(Arc<dyn SandboxAdapter>);
pub struct AgentAttachmentService {
    access: Arc<ResourceAccessService>,
    writer: Arc<dyn AttachmentWriter>,
}
impl AgentAttachmentService {
    pub fn new(access: Arc<ResourceAccessService>, adapter: Arc<dyn SandboxAdapter>) -> Arc<Self> {
        Self::with_writer(access, Arc::new(RuntimeAttachmentWriter(adapter)))
    }
    pub fn with_writer(
        access: Arc<ResourceAccessService>,
        writer: Arc<dyn AttachmentWriter>,
    ) -> Arc<Self> {
        Arc::new(Self { access, writer })
    }
    /// Authorize before reading bytes. Sharing or delegated conversation rights do not permit writes.
    pub async fn admit(
        &self,
        caller: &Caller,
        context: Option<&CallerContext>,
        credential: Option<&str>,
        agent_id: &AgentId,
        metadata: AttachmentMetadata<'_>,
    ) -> Result<AttachmentAdmission, AttachmentError> {
        let AttachmentMetadata {
            key,
            name,
            media_type,
        } = metadata;
        let name = sanitize_name(name)?;
        if key.is_empty()
            || key.len() > 255
            || !key.bytes().all(|b| (0x21..=0x7e).contains(&b))
            || !valid_media_type(media_type)
        {
            return Err(AttachmentError::Invalid);
        }
        let target = ResourceTarget::Agent {
            agent_id: agent_id.clone(),
        };
        let authorized = if let Some(context) = context {
            if caller != &context.caller
                || !context.validate_principal()
                || !matches!(&context.actor, ResourceActor::Human { subject } | ResourceActor::Api { subject, delegation_id: None } if subject == &caller.subject)
            {
                return Err(ResourceAccessError::from(AccessError::NotFound).into());
            }
            self.access
                .authorize(
                    context,
                    credential,
                    &target,
                    ResourceCapability::ManageSharing,
                    AccessPurpose::Mutation {
                        started_at: SystemTime::now(),
                    },
                )
                .await?
        } else {
            self.access
                .authorize_personal(caller, &target, ResourceCapability::ManageSharing)
                .await?
        };
        available(&authorized.snapshot)?;
        let identity = identity(&authorized.snapshot)?;
        let id = format!(
            "att_{:x}",
            Sha256::digest(
                serde_json::to_vec(&("agent-attachment-v1", &identity, key))
                    .map_err(|_| AttachmentError::Invalid)?
            )
        );
        Ok(AttachmentAdmission {
            authorized,
            target,
            identity,
            id,
            name,
            media_type: media_type.into(),
            deadline: tokio::time::Instant::now() + ATTACHMENT_TIMEOUT,
        })
    }
    pub async fn upload(
        &self,
        admission: AttachmentAdmission,
        bytes: Vec<u8>,
    ) -> Result<AgentAttachment, AttachmentError> {
        if bytes.len() > MAX_ATTACHMENT_BYTES {
            return Err(AttachmentError::TooLarge);
        }
        tokio::time::timeout_at(admission.deadline, self.upload_inner(admission, bytes))
            .await
            .map_err(|_| AttachmentError::Unavailable)?
    }
    async fn current(
        &self,
        admission: &AttachmentAdmission,
    ) -> Result<ResourceAccessSnapshot, AttachmentError> {
        let current = self
            .access
            .current_read_snapshot(
                &admission.authorized,
                &admission.target,
                ResourceCapability::ManageSharing,
            )
            .await?;
        available(&current)?;
        if identity(&current)? != admission.identity
            || current.lineage.sandbox.runtime
                != admission.authorized.snapshot.lineage.sandbox.runtime
            || current.lineage.sandbox.lifecycle.home
                != admission.authorized.snapshot.lineage.sandbox.lifecycle.home
        {
            return Err(AttachmentError::Conflict);
        }
        Ok(current)
    }
    async fn upload_inner(
        &self,
        admission: AttachmentAdmission,
        bytes: Vec<u8>,
    ) -> Result<AgentAttachment, AttachmentError> {
        let current = self.current(&admission).await?;
        let agent = current
            .lineage
            .agent
            .as_ref()
            .ok_or(AttachmentError::NotReady)?;
        let attachment = AgentAttachment {
            id: admission.id.clone(),
            agent_id: agent.id.clone(),
            name: admission.name.clone(),
            path: format!(
                "{}/{}/{}/{}",
                layer3_adapter::AGENT_ATTACHMENT_DIRECTORY,
                agent.id,
                admission.id,
                admission.name
            ),
            size_bytes: bytes.len() as u64,
            media_type: admission.media_type.clone(),
            sha256: format!("{:x}", Sha256::digest(&bytes)),
        };
        let receipt = self
            .writer
            .write(&current.lineage.sandbox, &attachment, bytes)
            .await?;
        if receipt != attachment {
            return Err(AttachmentError::Conflict);
        }
        self.current(&admission).await?;
        Ok(receipt)
    }
}
#[async_trait::async_trait]
impl AttachmentWriter for RuntimeAttachmentWriter {
    async fn write(
        &self,
        sandbox: &Sandbox,
        attachment: &AgentAttachment,
        content: Vec<u8>,
    ) -> Result<AgentAttachment, AttachmentError> {
        let runtime = sandbox.runtime.as_ref().ok_or(AttachmentError::NotReady)?;
        let home = sandbox
            .lifecycle
            .home
            .as_ref()
            .ok_or(AttachmentError::Unsupported)?;
        if home.released || home.receipt.is_none() || !home.request.policy.mounts_full_home() {
            return Err(AttachmentError::Unsupported);
        }
        let generation = runtime
            .provision_generation
            .ok_or(AttachmentError::Unsupported)?;
        let handle = RuntimeHandle(runtime.runtime_id.clone());
        let status_request = RuntimeStatusRequest {
            sandbox_id: sandbox.id.to_string(),
            generation,
            home_identity: home
                .attachment()
                .ok_or(AttachmentError::Unsupported)?
                .identity()?,
        };
        let status = self.0.runtime_status(&handle).await?;
        status.validate_for(&status_request)?;
        if !status.agent_attachment_upload {
            return Err(AttachmentError::Unsupported);
        }
        if runtime.boot_id.as_deref() != Some(status.boot_id.as_str())
            || !status.installation_ready
            || !status.credential_ready
            || status.restore_pending
        {
            return Err(AttachmentError::NotReady);
        }
        let request = AgentAttachmentRequest {
            sandbox_id: sandbox.id.to_string(),
            runtime_generation: generation,
            boot_id: status.boot_id,
            agent_id: AgentSlot(attachment.agent_id.to_string()),
            attachment_id: attachment.id.clone(),
            name: attachment.name.clone(),
            media_type: attachment.media_type.clone(),
            size_bytes: attachment.size_bytes,
            sha256: attachment.sha256.clone(),
        };
        request.validate_content(&content)?;
        let receipt = self
            .0
            .upload_agent_attachment(&handle, &request, content)
            .await?;
        receipt.validate_for(&request)?;
        if receipt.path != attachment.path {
            return Err(AttachmentError::Conflict);
        }
        Ok(attachment.clone())
    }
}

fn available(snapshot: &ResourceAccessSnapshot) -> Result<(), AttachmentError> {
    let lineage = &snapshot.lineage;
    if lineage.sandbox.status != SandboxStatus::Active
        || lineage.sandbox.lifecycle.operation.is_some()
        || !lineage.agent.as_ref().is_some_and(|a| {
            matches!(
                a.status,
                AgentStatus::Ready | AgentStatus::Working | AgentStatus::WaitingApproval
            ) && a.deleted_at.is_none()
                && a.requested_stop_reason.is_none()
                && a.stopped_at.is_none()
        })
        || !lineage
            .worktree
            .as_ref()
            .is_some_and(|w| w.status == WorktreeStatus::Ready)
        || !lineage
            .workspace
            .as_ref()
            .is_some_and(|w| w.status == WorkspaceStatus::Ready)
    {
        return Err(AttachmentError::NotReady);
    }
    Ok(())
}
fn identity(snapshot: &ResourceAccessSnapshot) -> Result<Vec<u8>, AttachmentError> {
    let lineage = &snapshot.lineage;
    let agent = lineage.agent.as_ref().ok_or(AttachmentError::Conflict)?;
    let workspace = lineage
        .workspace
        .as_ref()
        .ok_or(AttachmentError::Conflict)?;
    let worktree = lineage.worktree.as_ref().ok_or(AttachmentError::Conflict)?;
    let scope = snapshot
        .scope(&ResourceTarget::Agent {
            agent_id: agent.id.clone(),
        })
        .map_err(ResourceAccessError::from)?;
    serde_json::to_vec(&(
        &scope.owner,
        &lineage.sandbox.id,
        &lineage.sandbox.created_at,
        &workspace.id,
        &workspace.created_at,
        &worktree.id,
        &worktree.path,
        &worktree.branch,
        &worktree.base_commit,
        &agent.id,
        &agent.created_at,
        &agent.creation_fingerprint,
    ))
    .map_err(|_| ResourceAccessError::from(ResourceAccessStoreError::Corrupt).into())
}
/// Preserve ordinary extensions while producing one ASCII, non-hidden filesystem component.
pub fn sanitize_name(value: &str) -> Result<String, AttachmentError> {
    if value.is_empty()
        || value.len() > 1024
        || matches!(value, "." | "..")
        || value
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\'))
    {
        return Err(AttachmentError::Invalid);
    }
    let mut name: String = value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .take(120)
        .collect();
    if name.starts_with('.') {
        name.replace_range(..1, "_");
    }
    if name.is_empty() {
        return Err(AttachmentError::Invalid);
    }
    Ok(name)
}
pub fn valid_media_type(value: &str) -> bool {
    value.len() <= 127
        && value.split_once('/').is_some_and(|(kind, subtype)| {
            !kind.is_empty()
                && !subtype.is_empty()
                && kind
                    .bytes()
                    .chain(subtype.bytes())
                    .all(|b| b.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&b))
        })
}
