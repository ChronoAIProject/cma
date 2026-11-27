//! Existing supervisor file operations bound to one Agent's retained home.
use super::acp::shared::Shared;
use crate::ingress_budget::diagnostic::text;
use binding::Initial;
use std::sync::Arc;

pub(in crate::hosted) mod binding;

pub(in crate::hosted) mod guidance_admission;
pub(in crate::hosted) mod instructions_admission;
mod receipt;
pub(in crate::hosted) mod receipt_admission;
pub(in crate::hosted) mod transport;
pub(in crate::hosted) mod transport_admission;
mod transport_goal;
mod transport_goal_admission;

#[cfg(test)]
#[path = "managed_context/receipt_tests.rs"]
mod receipt_tests;

#[cfg(test)]
#[path = "managed_context/binding_storage_tests.rs"]
mod binding_storage_tests;

#[cfg(test)]
#[path = "managed_context/binding_membership_tests.rs"]
mod binding_membership_tests;

#[cfg(test)]
pub(in crate::hosted) mod binding_fixture;

use base64::Engine as _;
use sha2::{Digest as _, Sha256};

use super::HostedBackend;
use crate::wire::{RequestFrame, ResponseFrame};
use crate::{AdapterError, RuntimeHandle, RuntimeStatus, RuntimeStatusRequest};

#[derive(Clone)]
pub struct HostedContextRefresh(Shared<Binding>);
struct Binding {
    backend: Shared<dyn HostedBackend>,
    handle: RuntimeHandle,
    identity: RuntimeStatusRequest,
    path: String,
    cwd: String,
    tools: Vec<serde_json::Value>,
    installed_transport: std::sync::RwLock<Option<transport::ToolTransport>>,
    access: Option<(
        Shared<dyn super::HostedAccessToolSource>,
        Initial<super::HostedAccessToolBinding>,
    )>,
    provider: Option<(
        Shared<dyn super::HostedProviderToolSource>,
        Initial<super::HostedProviderToolBinding>,
    )>,
}
impl std::fmt::Debug for HostedContextRefresh {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostedContextRefresh")
            .field("path", &self.0.path)
            .finish_non_exhaustive()
    }
}
impl PartialEq for HostedContextRefresh {
    fn eq(&self, other: &Self) -> bool {
        self.0.ptr_eq(&other.0)
    }
}
impl Eq for HostedContextRefresh {}

impl HostedContextRefresh {
    pub(super) fn new(
        backend: Arc<dyn HostedBackend>,
        handle: RuntimeHandle,
        identity: RuntimeStatusRequest,
        home: &str,
        cwd: String,
        tools: Vec<serde_json::Value>,
    ) -> crate::Result<Self> {
        let path = home
            .strip_prefix("/home/")
            .filter(|p| crate::valid_agent_file_path(p))
            .ok_or_else(|| {
                AdapterError::Unsupported(text(
                    "Agent instructions require a retained complete home",
                ))
            })?;
        identity.validate()?;
        Ok(Self(Shared::Legacy(Arc::new(Binding {
            backend: Shared::Legacy(backend),
            handle,
            identity,
            path: format!("{path}/AGENTS.md"),
            cwd,
            tools,
            installed_transport: std::sync::RwLock::new(None),
            access: None,
            provider: None,
        }))))
    }

    pub(super) fn with_access(
        mut self,
        source: Arc<dyn super::HostedAccessToolSource>,
        binding: super::HostedAccessToolBinding,
    ) -> Self {
        self.legacy_mut().access = Some((Shared::Legacy(source), Initial::Legacy(binding)));
        self
    }
    pub(super) fn cwd(&self) -> &str {
        &self.0.cwd
    }

    pub(super) fn with_provider(
        mut self,
        source: Arc<dyn super::HostedProviderToolSource>,
        binding: super::HostedProviderToolBinding,
    ) -> Self {
        self.legacy_mut().provider = Some((Shared::Legacy(source), Initial::Legacy(binding)));
        self
    }

    fn legacy_mut(&mut self) -> &mut Binding {
        let Shared::Legacy(binding) = &mut self.0 else {
            panic!("admitted context bindings are immutable")
        };
        Arc::get_mut(binding).expect("new context binding is uniquely owned")
    }
    pub(super) fn tools(&self) -> Vec<serde_json::Value> {
        self.tools_for_transport(None)
            .expect("confirmed transport has one managed server")
    }

    async fn exchange(&self, request: RequestFrame) -> crate::Result<ResponseFrame> {
        let operation = request.operation();
        let response = self.0.backend.exchange(&self.0.handle, request).await?;
        match response {
            ResponseFrame::Failure(failure) if failure.operation == operation => {
                Err(super::reply::failure_error(failure))
            }
            response if super::reply::response_operation(&response) == operation => Ok(response),
            _ => Err(AdapterError::unexpected_reply("managed context")),
        }
    }
    async fn status(&self) -> crate::Result<RuntimeStatus> {
        let ResponseFrame::RuntimeStatus(status) = self
            .exchange(RequestFrame::RuntimeStatus(self.0.identity.clone()))
            .await?
        else {
            return Err(AdapterError::unexpected_reply("managed context runtime"));
        };
        status.validate_for(&self.0.identity)?;
        if !status.agent_filesystem || !status.credential_ready || status.restore_pending {
            return Err(AdapterError::Unsupported(text(
                "runtime is not ready for managed context refresh",
            )));
        }
        Ok(status)
    }
    pub(super) async fn readiness_receipt(
        &self,
        session: &str,
        sequence: u64,
    ) -> crate::Result<String> {
        let status = self.status().await?;
        Ok(self.receipt(&status, session, sequence))
    }

    /// Confirm the failed instruction operation persisted exactly its desired
    /// bytes in this runtime generation before reloading the retained thread.
    pub(super) async fn confirm_instructions(&self, expected: &str) -> crate::Result<()> {
        let status = self.status().await?;
        let ResponseFrame::ReadAgentFile(file) = self
            .exchange(RequestFrame::ReadAgentFile(crate::AgentFileReadRequest {
                owner_access: true,
                protected_paths: Vec::new(),
                sandbox_id: status.sandbox_id,
                runtime_generation: status.generation,
                boot_id: status.boot_id,
                home_identity: None,
                path: self.0.path.clone(),
                max_bytes: 32 * 1024,
            }))
            .await?
        else {
            return Err(AdapterError::unexpected_reply("retained instructions read"));
        };
        if file.path != self.0.path || file.content()? != expected.as_bytes() {
            return Err(AdapterError::OperationIdentityConflict(text(
                "retained instructions do not match the pending settings operation",
            )));
        }
        Ok(())
    }
    /// Add only the managed block. The supervisor's existing descriptor-confined
    /// compare-and-swap preserves edits and rejects replacement boots or paths.
    pub(super) async fn append_guidance(
        &self,
        session: &str,
        sequence: u64,
        readiness_receipt: &str,
        guidance: &str,
        previous: &[String],
    ) -> crate::Result<()> {
        if guidance.is_empty() || guidance.len() > 16 * 1024 || guidance.contains('\0') {
            return Err(AdapterError::Unsupported(text(
                "managed guidance is invalid",
            )));
        }
        let status = self.status().await?;
        if sequence == 0 || self.receipt(&status, session, sequence) != readiness_receipt {
            return Err(AdapterError::OperationIdentityConflict(text(
                "managed context readiness belongs to another runtime identity",
            )));
        }
        // This is control-plane maintenance, not shared filesystem access. The
        // binding fixes the owning runtime and exact Agent-home path before any
        // input is accepted; owner_access permits only that internal file here.
        let ResponseFrame::ReadAgentFile(file) = self
            .exchange(RequestFrame::ReadAgentFile(crate::AgentFileReadRequest {
                owner_access: true,
                protected_paths: Vec::new(),
                sandbox_id: status.sandbox_id.clone(),
                runtime_generation: status.generation,
                boot_id: status.boot_id.clone(),
                home_identity: None,
                path: self.0.path.clone(),
                max_bytes: 32 * 1024,
            }))
            .await?
        else {
            return Err(AdapterError::unexpected_reply("managed instructions read"));
        };
        if file.path != self.0.path {
            return Err(AdapterError::unexpected_reply("managed instructions path"));
        }
        let original = String::from_utf8(file.content()?)
            .map_err(|_| AdapterError::Rejected(text("Agent instructions are not UTF-8")))?;
        let Some(updated) = replace_managed_prefix(&original, guidance, previous)? else {
            return Ok(());
        };
        let sha = format!("{:x}", Sha256::digest(updated.as_bytes()));
        let ResponseFrame::WriteAgentFile(receipt) = self
            .exchange(RequestFrame::WriteAgentFile(crate::AgentFileWriteRequest {
                owner_access: true,
                protected_paths: Vec::new(),
                sandbox_id: status.sandbox_id,
                runtime_generation: status.generation,
                boot_id: status.boot_id,
                path: self.0.path.clone(),
                expected_sha256: Some(file.sha256),
                content_base64: base64::engine::general_purpose::STANDARD
                    .encode(updated.as_bytes()),
                directory: false,
            }))
            .await?
        else {
            return Err(AdapterError::unexpected_reply("managed instructions write"));
        };
        if receipt.path != self.0.path
            || receipt.sha256 != sha
            || receipt.size_bytes != updated.len() as u64
        {
            return Err(AdapterError::Rejected(text(
                "runtime did not confirm managed instructions",
            )));
        }
        Ok(())
    }
}

fn replace_managed_prefix(
    original: &str,
    guidance: &str,
    previous: &[String],
) -> crate::Result<Option<String>> {
    if previous.len() > 8
        || previous
            .iter()
            .any(|prefix| prefix.is_empty() || prefix.len() > 16 * 1024 || prefix.contains('\0'))
    {
        return Err(AdapterError::Unsupported(text(
            "managed guidance history is invalid",
        )));
    }
    let owner = previous
        .iter()
        .map(String::as_str)
        .chain(std::iter::once(guidance))
        .filter_map(|prefix| {
            if original == prefix {
                Some((prefix.len(), ""))
            } else {
                original
                    .strip_prefix(prefix)
                    .and_then(|suffix| suffix.strip_prefix("\n\n"))
                    .map(|owner| (prefix.len(), owner))
            }
        })
        .max_by_key(|(len, _)| *len)
        .map_or(original, |(_, owner)| owner);
    let bytes = guidance
        .len()
        .checked_add(owner.len())
        .and_then(|bytes| bytes.checked_add(if owner.is_empty() { 0 } else { 2 }));
    let Some(bytes) = bytes.filter(|bytes| *bytes <= 32 * 1024) else {
        return Err(AdapterError::Unsupported(text(
            "Agent instructions have no capacity for managed guidance",
        )));
    };
    let mut updated = String::with_capacity(bytes);
    updated.push_str(guidance);
    if !owner.is_empty() {
        updated.push_str("\n\n");
        updated.push_str(owner);
    }
    Ok((updated != original).then_some(updated))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn prefix_replacement_preserves_exact_owner_bytes_and_replay_does_not_duplicate() {
        let owner = "Owner instructions.  \n";
        let old = "Managed v1";
        let old_long = "Managed v1\n\nManaged v2";
        let new = "Managed v1\n\nManaged v2\n\nManaged document tools.";
        let previous = vec![old.into(), old_long.into(), new.into()];
        for original in [
            owner.to_owned(),
            format!("{old}\n\n{owner}"),
            format!("{old_long}\n\n{owner}"),
        ] {
            assert_eq!(
                replace_managed_prefix(&original, new, &previous).unwrap(),
                Some(format!("{new}\n\n{owner}"))
            );
        }
        assert_eq!(
            replace_managed_prefix(&format!("{new}\n\n{owner}"), new, &previous).unwrap(),
            None
        );
        assert_eq!(replace_managed_prefix(new, new, &previous).unwrap(), None);
        assert_eq!(
            replace_managed_prefix("Managed v1 quoted in user text", new, &previous).unwrap(),
            Some(format!("{new}\n\nManaged v1 quoted in user text"))
        );
        assert!(replace_managed_prefix(&"x".repeat(32 * 1024), new, &previous).is_err());
    }
}

#[cfg(test)]
#[path = "managed_context_tests.rs"]
pub(super) mod writer_tests;
