//! Codex-specific credential custody added at the trusted adapter boundary.
pub(super) fn protect_provider_credentials(owner_access: bool, protected_paths: &mut Vec<String>) {
    if !owner_access {
        protected_paths.extend([".codex/auth.json".into(), "cma/.codex/auth.json".into()]);
    }
}

use super::*;

impl CodexAdapter {
    pub(super) async fn list_agent_files_protected(
        &self,
        handle: &RuntimeHandle,
        request: &crate::AgentFileListRequest,
    ) -> crate::Result<crate::AgentFileListReceipt> {
        let mut request = request.clone();
        protect_provider_credentials(request.owner_access, &mut request.protected_paths);
        self.hosted.list_agent_files(handle, &request).await
    }

    pub(super) async fn read_agent_file_protected(
        &self,
        handle: &RuntimeHandle,
        request: &crate::AgentFileReadRequest,
    ) -> crate::Result<crate::AgentFileReadReceipt> {
        let mut request = request.clone();
        protect_provider_credentials(request.owner_access, &mut request.protected_paths);
        self.hosted.read_agent_file(handle, &request).await
    }

    pub(super) async fn write_agent_file_protected(
        &self,
        handle: &RuntimeHandle,
        request: &crate::AgentFileWriteRequest,
    ) -> crate::Result<crate::AgentFileWriteReceipt> {
        let mut request = request.clone();
        protect_provider_credentials(request.owner_access, &mut request.protected_paths);
        self.hosted.write_agent_file(handle, &request).await
    }

    pub(super) async fn delete_agent_file_protected(
        &self,
        handle: &RuntimeHandle,
        request: &crate::AgentFileDeleteRequest,
    ) -> crate::Result<crate::AgentFileDeleteReceipt> {
        let mut request = request.clone();
        protect_provider_credentials(request.owner_access, &mut request.protected_paths);
        self.hosted.delete_agent_file(handle, &request).await
    }
}
