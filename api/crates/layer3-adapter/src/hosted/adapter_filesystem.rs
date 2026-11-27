use super::HostedAdapter;
use crate::file_read_diagnostics::{measure, Stage};
use crate::wire::{RequestFrame, ResponseFrame};
use crate::{AdapterError, RuntimeHandle};

#[async_trait::async_trait]
impl crate::BrowserAdapter for HostedAdapter {
    async fn browser_control(
        &self,
        handle: &RuntimeHandle,
        request: &crate::BrowserControlRequest,
    ) -> crate::Result<crate::BrowserControlReceipt> {
        let state = self.known_state(handle).await?;
        validate_identity(
            &state,
            &request.sandbox_id,
            request.runtime_generation,
            &request.boot_id,
        )?;
        if !request.valid() {
            return Err(AdapterError::Rejected("browser_control_invalid".into()));
        }
        let status = self.inspect_runtime_status(handle).await?;
        if !status.agent_browser {
            return Err(AdapterError::Unsupported(
                "browser control is unavailable".into(),
            ));
        }
        if status.boot_id != request.boot_id || !status.credential_ready || status.restore_pending {
            return Err(AdapterError::Rejected(
                "browser runtime identity changed".into(),
            ));
        }
        match self
            .exchange(handle, RequestFrame::BrowserControl(request.clone()))
            .await?
        {
            ResponseFrame::BrowserControl(receipt) => Ok(receipt),
            _ => unreachable!("exchange verifies response operation"),
        }
    }

    async fn browser(
        &self,
        handle: &RuntimeHandle,
        request: &crate::BrowserRequest,
    ) -> crate::Result<crate::BrowserReceipt> {
        let state = self.known_state(handle).await?;
        validate_identity(
            &state,
            &request.sandbox_id,
            request.runtime_generation,
            &request.boot_id,
        )?;
        let status = self.inspect_runtime_status(handle).await?;
        if !status.agent_browser {
            return Err(AdapterError::Unsupported(
                "runtime browser observation is unavailable".into(),
            ));
        }
        if status.boot_id != request.boot_id || !status.credential_ready || status.restore_pending {
            return Err(AdapterError::Rejected(
                "browser runtime identity changed".into(),
            ));
        }
        match self
            .exchange(handle, RequestFrame::Browser(request.clone()))
            .await?
        {
            ResponseFrame::Browser(receipt) => Ok(receipt),
            _ => unreachable!("exchange verifies response operation"),
        }
    }
}

fn validate_identity(
    state: &super::RuntimeState,
    sandbox_id: &str,
    generation: u64,
    boot_id: &str,
) -> crate::Result<()> {
    if !crate::agent_filesystem::valid_identity(sandbox_id, generation, boot_id)
        || state.provision.sandbox_id != sandbox_id
        || state.provision.generation != generation
    {
        return Err(AdapterError::Rejected(
            "filesystem request does not name this runtime".into(),
        ));
    }
    Ok(())
}

impl HostedAdapter {
    pub(super) async fn list_files(
        &self,
        handle: &RuntimeHandle,
        request: &crate::AgentFileListRequest,
    ) -> crate::Result<crate::AgentFileListReceipt> {
        if !crate::valid_agent_file_path(&request.path) {
            return Err(AdapterError::Rejected("filesystem path is invalid".into()));
        }
        let state = self.known_state(handle).await?;
        let home = state.home_volume.as_ref().ok_or_else(unsupported)?;
        if !home.request.policy.mounts_full_home() {
            return Err(unsupported());
        }
        let status = self.inspect_runtime_status(handle).await?;
        validate_identity(
            &state,
            &request.sandbox_id,
            request.runtime_generation,
            &request.boot_id,
        )?;
        if !status.agent_filesystem {
            return Err(unsupported());
        }
        if status.boot_id != request.boot_id || !status.credential_ready || status.restore_pending {
            return Err(AdapterError::Rejected(
                "runtime is not ready for filesystem access".into(),
            ));
        }
        let response = self
            .exchange(handle, RequestFrame::ListAgentFiles(request.clone()))
            .await?;
        match response {
            ResponseFrame::ListAgentFiles(receipt) => Ok(receipt),
            _ => unreachable!("exchange verifies response operation"),
        }
    }

    pub(super) async fn read_file(
        &self,
        handle: &RuntimeHandle,
        request: &crate::AgentFileReadRequest,
    ) -> crate::Result<crate::AgentFileReadReceipt> {
        if !crate::valid_agent_file_path(&request.path)
            || request.max_bytes == 0
            || request.max_bytes > crate::MAX_AGENT_FILE_BYTES
        {
            return Err(AdapterError::Rejected(
                "filesystem read request is invalid".into(),
            ));
        }
        let state = self.known_state(handle).await?;
        let home = state.home_volume.as_ref().ok_or_else(unsupported)?;
        if !home.request.policy.mounts_full_home() {
            return Err(unsupported());
        }
        validate_identity(
            &state,
            &request.sandbox_id,
            request.runtime_generation,
            &request.boot_id,
        )?;
        if let Some(receipt) = self
            .backend
            .read_file_with_binding(handle, home, request)
            .await?
        {
            return Ok(receipt);
        }
        let status = measure(Stage::RuntimeStatus, self.inspect_runtime_status(handle)).await?;
        if !status.agent_filesystem {
            return Err(unsupported());
        }
        if status.boot_id != request.boot_id || !status.credential_ready || status.restore_pending {
            return Err(AdapterError::Rejected(
                "runtime is not ready for filesystem access".into(),
            ));
        }
        let response = measure(
            Stage::RuntimeRead,
            self.exchange(handle, RequestFrame::ReadAgentFile(request.clone())),
        )
        .await?;
        match response {
            ResponseFrame::ReadAgentFile(receipt) => {
                measure(Stage::ReceiptValidation, async { receipt.content() }).await?;
                Ok(receipt)
            }
            _ => unreachable!("exchange verifies response operation"),
        }
    }

    pub(super) async fn write_file(
        &self,
        handle: &RuntimeHandle,
        request: &crate::AgentFileWriteRequest,
    ) -> crate::Result<crate::AgentFileWriteReceipt> {
        if !crate::valid_agent_file_path(&request.path)
            || request.path.is_empty()
            || request
                .expected_sha256
                .as_ref()
                .is_some_and(|v| v.len() != 64 || !v.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err(AdapterError::Rejected(
                "filesystem write request is invalid".into(),
            ));
        }
        request.content()?;
        let state = self.known_state(handle).await?;
        let home = state.home_volume.as_ref().ok_or_else(unsupported)?;
        if !home.request.policy.mounts_full_home() {
            return Err(unsupported());
        }
        let status = self.inspect_runtime_status(handle).await?;
        validate_identity(
            &state,
            &request.sandbox_id,
            request.runtime_generation,
            &request.boot_id,
        )?;
        if !status.agent_filesystem {
            return Err(unsupported());
        }
        if status.boot_id != request.boot_id || !status.credential_ready || status.restore_pending {
            return Err(AdapterError::Rejected(
                "runtime is not ready for filesystem access".into(),
            ));
        }
        let response = self
            .exchange(handle, RequestFrame::WriteAgentFile(request.clone()))
            .await?;
        match response {
            ResponseFrame::WriteAgentFile(receipt) => Ok(receipt),
            _ => unreachable!("exchange verifies response operation"),
        }
    }

    pub(super) async fn delete_file(
        &self,
        handle: &RuntimeHandle,
        request: &crate::AgentFileDeleteRequest,
    ) -> crate::Result<crate::AgentFileDeleteReceipt> {
        if !crate::valid_agent_file_path(&request.path) || request.path.is_empty() {
            return Err(AdapterError::Rejected(
                "filesystem delete path is invalid".into(),
            ));
        }
        let state = self.known_state(handle).await?;
        let home = state.home_volume.as_ref().ok_or_else(unsupported)?;
        if !home.request.policy.mounts_full_home() {
            return Err(unsupported());
        }
        let status = self.inspect_runtime_status(handle).await?;
        validate_identity(
            &state,
            &request.sandbox_id,
            request.runtime_generation,
            &request.boot_id,
        )?;
        if !status.agent_filesystem {
            return Err(unsupported());
        }
        if status.boot_id != request.boot_id || !status.credential_ready || status.restore_pending {
            return Err(AdapterError::Rejected(
                "runtime is not ready for filesystem access".into(),
            ));
        }
        let response = self
            .exchange(handle, RequestFrame::DeleteAgentFile(request.clone()))
            .await?;
        match response {
            ResponseFrame::DeleteAgentFile(receipt) => Ok(receipt),
            _ => unreachable!("exchange verifies response operation"),
        }
    }
}

fn unsupported() -> AdapterError {
    AdapterError::Unsupported(
        "filesystem access requires a current runtime with the complete persistent home".into(),
    )
}
