//! Existing supervisor file operations bound to one Agent's retained home.
use std::sync::Arc;

use base64::Engine as _;
use sha2::{Digest as _, Sha256};

use super::HostedBackend;
use crate::wire::{RequestFrame, ResponseFrame};
use crate::{AdapterError, RuntimeHandle, RuntimeStatus, RuntimeStatusRequest};

#[derive(Clone)]
pub struct HostedContextRefresh(Arc<Binding>);
struct Binding {
    backend: Arc<dyn HostedBackend>,
    handle: RuntimeHandle,
    identity: RuntimeStatusRequest,
    path: String,
    cwd: String,
    tools: Vec<serde_json::Value>,
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
        Arc::ptr_eq(&self.0, &other.0)
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
                AdapterError::Unsupported(
                    "Agent instructions require a retained complete home".into(),
                )
            })?;
        identity.validate()?;
        Ok(Self(Arc::new(Binding {
            backend,
            handle,
            identity,
            path: format!("{path}/AGENTS.md"),
            cwd,
            tools,
        })))
    }
    pub(super) fn cwd(&self) -> &str {
        &self.0.cwd
    }
    pub(super) fn tools(&self) -> &[serde_json::Value] {
        &self.0.tools
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
            return Err(AdapterError::Unsupported(
                "runtime is not ready for managed context refresh".into(),
            ));
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
                path: self.0.path.clone(),
                max_bytes: 32 * 1024,
            }))
            .await?
        else {
            return Err(AdapterError::unexpected_reply("retained instructions read"));
        };
        if file.path != self.0.path || file.content()? != expected.as_bytes() {
            return Err(AdapterError::OperationIdentityConflict(
                "retained instructions do not match the pending settings operation".into(),
            ));
        }
        Ok(())
    }
    fn receipt(&self, status: &RuntimeStatus, session: &str, sequence: u64) -> String {
        let exact = serde_json::json!([
            "managed-context-v1",
            self.0.handle.0,
            status.sandbox_id,
            status.generation,
            status.home_identity,
            status.boot_id,
            self.0.path,
            session,
            sequence
        ]);
        format!("{:x}", Sha256::digest(exact.to_string().as_bytes()))
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
            return Err(AdapterError::Unsupported(
                "managed guidance is invalid".into(),
            ));
        }
        let status = self.status().await?;
        if sequence == 0 || self.receipt(&status, session, sequence) != readiness_receipt {
            return Err(AdapterError::OperationIdentityConflict(
                "managed context readiness belongs to another runtime identity".into(),
            ));
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
            .map_err(|_| AdapterError::Rejected("Agent instructions are not UTF-8".into()))?;
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
            return Err(AdapterError::Rejected(
                "runtime did not confirm managed instructions".into(),
            ));
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
        return Err(AdapterError::Unsupported(
            "managed guidance history is invalid".into(),
        ));
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
    let updated = if owner.is_empty() {
        guidance.to_owned()
    } else {
        format!("{guidance}\n\n{owner}")
    };
    if updated.len() > 32 * 1024 {
        return Err(AdapterError::Unsupported(
            "Agent instructions have no capacity for managed guidance".into(),
        ));
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
