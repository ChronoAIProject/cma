use super::*;
use tokio::sync::mpsc;
pub(crate) struct UnusedRuntime {
    pub(crate) status: Option<RuntimeStatus>,
    pub(crate) file_root: Option<(std::path::PathBuf, Arc<std::sync::Mutex<Vec<u64>>>)>,
}
#[async_trait::async_trait]
impl SandboxAdapter for UnusedRuntime {
    async fn list_agent_files(
        &self,
        _: &RuntimeHandle,
        request: &AgentFileListRequest,
    ) -> Result<AgentFileListReceipt> {
        Ok(AgentFileListReceipt {
            path: request.path.clone(),
            entries: vec![],
        })
    }
    async fn read_agent_file(
        &self,
        _: &RuntimeHandle,
        request: &AgentFileReadRequest,
    ) -> Result<AgentFileReadReceipt> {
        if let Some((root, reads)) = &self.file_root {
            reads.lock().unwrap().push(request.max_bytes);
            use base64::Engine as _;
            use sha2::{Digest as _, Sha256};
            let path = root.join(&request.path);
            let metadata = std::fs::metadata(&path)
                .map_err(|_| AdapterError::Rejected("file_not_found: fixture file".into()))?;
            if metadata.len() > request.max_bytes {
                return Err(AdapterError::Rejected(
                    "file_too_large: fixture file".into(),
                ));
            }
            let bytes = std::fs::read(path).unwrap();
            return Ok(AgentFileReadReceipt {
                path: request.path.clone(),
                size_bytes: bytes.len() as u64,
                sha256: format!("{:x}", Sha256::digest(&bytes)),
                content_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
            });
        }
        Ok(AgentFileReadReceipt {
            path: request.path.clone(),
            size_bytes: 4,
            sha256: "3b9c358f36f0a31b6ad3e14f309c7cf198ac9246e8316f9ce543d5b19ac02b80".into(),
            content_base64: "ZmlsZQ==".into(),
        })
    }
    async fn write_agent_file(
        &self,
        _: &RuntimeHandle,
        request: &AgentFileWriteRequest,
    ) -> Result<AgentFileWriteReceipt> {
        use sha2::{Digest as _, Sha256};
        let bytes = request.content()?;
        Ok(AgentFileWriteReceipt {
            path: request.path.clone(),
            size_bytes: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(bytes)),
        })
    }
    async fn delete_agent_file(
        &self,
        _: &RuntimeHandle,
        request: &AgentFileDeleteRequest,
    ) -> Result<AgentFileDeleteReceipt> {
        Ok(AgentFileDeleteReceipt {
            path: request.path.clone(),
            deleted: true,
        })
    }
    async fn runtime_status(&self, handle: &RuntimeHandle) -> Result<RuntimeStatus> {
        assert_eq!(handle.as_str(), "runtime-test");
        Ok(self.status.clone().expect("unexpected runtime status"))
    }
    async fn upload_agent_attachment(
        &self,
        handle: &RuntimeHandle,
        request: &AgentAttachmentRequest,
        content: Vec<u8>,
    ) -> Result<AgentAttachmentReceipt> {
        assert_eq!(handle.as_str(), "runtime-test");
        let status = self.status.as_ref().unwrap();
        assert_eq!(request.boot_id, status.boot_id);
        assert_eq!(request.sandbox_id, status.sandbox_id);
        assert_eq!(request.runtime_generation, status.generation);
        assert_eq!(request.agent_id.as_str(), "agt_test");
        assert!(status.agent_attachment_upload);
        request.validate_content(&content)?;
        Ok(AgentAttachmentReceipt::for_request(request))
    }
    fn capabilities(&self) -> AdapterCapabilities {
        panic!("unexpected runtime effect")
    }
    async fn provision(&self, _spec: &SandboxSpec) -> Result<RuntimeHandle> {
        panic!("unexpected runtime effect")
    }
    async fn run_install(
        &self,
        _handle: &RuntimeHandle,
        _request: &InstallRequest,
    ) -> Result<InstallReceipt> {
        panic!("unexpected runtime effect")
    }
    async fn deliver_credential(
        &self,
        _handle: &RuntimeHandle,
        _frame: CredentialFrame,
    ) -> Result<()> {
        panic!("unexpected runtime effect")
    }
    async fn prepare_workspace(
        &self,
        _handle: &RuntimeHandle,
        _request: &WorkspaceRequest,
    ) -> Result<WorkspaceReceipt> {
        panic!("unexpected runtime effect")
    }
    async fn run_workspace_setup(
        &self,
        _handle: &RuntimeHandle,
        _request: &WorkspaceSetupRequest,
    ) -> Result<SetupReceipt> {
        panic!("unexpected runtime effect")
    }
    async fn remove_workspace(
        &self,
        _handle: &RuntimeHandle,
        _request: &WorkspaceRemoveRequest,
    ) -> Result<()> {
        panic!("unexpected runtime effect")
    }
    async fn add_worktree(
        &self,
        _handle: &RuntimeHandle,
        _authority_fence: u64,
        _request: &WorktreeRequest,
    ) -> Result<WorktreeReceipt> {
        panic!("unexpected runtime effect")
    }
    async fn remove_worktree(
        &self,
        _handle: &RuntimeHandle,
        _authority_fence: u64,
        _request: &WorktreeRef,
    ) -> Result<()> {
        panic!("unexpected runtime effect")
    }
    async fn materialize_agent_home(
        &self,
        _handle: &RuntimeHandle,
        _authority_fence: u64,
        _request: &AgentHomeRequest,
    ) -> Result<()> {
        panic!("unexpected runtime effect")
    }
    async fn remove_agent_home(
        &self,
        _handle: &RuntimeHandle,
        _authority_fence: u64,
        _request: &AgentHomeRemoveRequest,
    ) -> Result<()> {
        panic!("unexpected runtime effect")
    }
    async fn open_agent(
        &self,
        _handle: &RuntimeHandle,
        _authority_fence: u64,
        _request: &AgentOpenRequest,
        _updates: mpsc::Sender<ConversationUpdate>,
    ) -> Result<ConversationChannel> {
        panic!("unexpected runtime effect")
    }
    async fn reconnect_agent(
        &self,
        _handle: &RuntimeHandle,
        _authority_fence: u64,
        _request: &AgentOpenRequest,
        _conversation_id: &str,
        _updates: mpsc::Sender<ConversationUpdate>,
    ) -> Result<ConversationChannel> {
        panic!("unexpected runtime effect")
    }
    async fn close_agent(
        &self,
        _handle: &RuntimeHandle,
        _authority_fence: u64,
        _request: &AgentCloseRequest,
    ) -> Result<()> {
        panic!("unexpected runtime effect")
    }
    async fn capture_agent(
        &self,
        _handle: &RuntimeHandle,
        _agent: &AgentSlot,
    ) -> Result<ConversationSnapshot> {
        panic!("unexpected runtime effect")
    }
    async fn checkpoint(
        &self,
        _handle: &RuntimeHandle,
        _request: &CheckpointRequest,
    ) -> Result<CheckpointReceipt> {
        panic!("unexpected runtime effect")
    }
    async fn restore(&self, _handle: &RuntimeHandle, _request: &RestoreRequest) -> Result<()> {
        panic!("unexpected runtime effect")
    }
    async fn resources(&self, _handle: &RuntimeHandle) -> Result<ResourceReading> {
        panic!("unexpected runtime effect")
    }
    async fn teardown(&self, _handle: &RuntimeHandle) -> Result<()> {
        panic!("unexpected runtime effect")
    }
}
