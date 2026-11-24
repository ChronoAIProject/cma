use layer3_adapter::*;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, Notify};

pub(super) const FILE_NAME: &str = "handoff.txt";
pub(super) const FILE_PATH: &str = "cma/handoff.txt";
pub(super) const FILE_BYTES: &[u8] = b"fixture file survives owner handoff\n";
pub(super) const HANDLE: &str = "file-handoff-runtime";
pub(super) const SANDBOX: &str = "sbx_file_handoff";

#[derive(Default)]
pub(super) struct Files {
    pub reads: AtomicUsize,
    pub unavailable: AtomicBool,
    pub pause_next: AtomicBool,
    pub entered: Notify,
    pub resume: Notify,
    pub writes: AtomicUsize,
    pub lose_write_ack: AtomicBool,
    contents: Option<Arc<Mutex<Vec<u8>>>>,
}

impl Files {
    pub(super) fn with_write_contents(contents: Option<Arc<Mutex<Vec<u8>>>>) -> Self {
        Self {
            contents,
            ..Self::default()
        }
    }

    fn check(
        &self,
        handle: &RuntimeHandle,
        sandbox: &str,
        generation: u64,
        boot: &str,
    ) -> Result<()> {
        assert_eq!(handle.as_str(), HANDLE);
        assert_eq!(sandbox, SANDBOX);
        assert_eq!(generation, 1);
        assert_eq!(boot, "a".repeat(32));
        self.reads.fetch_add(1, Ordering::SeqCst);
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(AdapterError::Unavailable(
                "fixture runtime unavailable".into(),
            ));
        }
        Ok(())
    }
}

pub(super) fn digest() -> String {
    format!("{:x}", Sha256::digest(FILE_BYTES))
}

#[async_trait::async_trait]
impl SandboxAdapter for Files {
    async fn list_agent_files(
        &self,
        handle: &RuntimeHandle,
        request: &AgentFileListRequest,
    ) -> Result<AgentFileListReceipt> {
        self.check(
            handle,
            &request.sandbox_id,
            request.runtime_generation,
            &request.boot_id,
        )?;
        assert_eq!(request.path, "cma");
        assert!(request.owner_access);
        Ok(AgentFileListReceipt {
            path: request.path.clone(),
            entries: vec![AgentFileEntry {
                path: FILE_PATH.into(),
                name: FILE_NAME.into(),
                kind: AgentFileKind::File,
                size_bytes: FILE_BYTES.len() as u64,
                sha256: Some(digest()),
                hidden: false,
            }],
        })
    }

    async fn read_agent_file(
        &self,
        handle: &RuntimeHandle,
        request: &AgentFileReadRequest,
    ) -> Result<AgentFileReadReceipt> {
        use base64::Engine as _;
        self.check(
            handle,
            &request.sandbox_id,
            request.runtime_generation,
            &request.boot_id,
        )?;
        assert_eq!(request.path, FILE_PATH);
        assert!(request.owner_access);
        assert!(request.max_bytes >= FILE_BYTES.len() as u64);
        if self.pause_next.swap(false, Ordering::SeqCst) {
            self.entered.notify_one();
            self.resume.notified().await;
        }
        let contents = self.contents.as_ref().map_or_else(
            || FILE_BYTES.to_vec(),
            |contents| contents.lock().unwrap().clone(),
        );
        Ok(AgentFileReadReceipt {
            path: request.path.clone(),
            size_bytes: contents.len() as u64,
            sha256: format!("{:x}", Sha256::digest(&contents)),
            content_base64: base64::engine::general_purpose::STANDARD.encode(&contents),
        })
    }

    async fn write_agent_file(
        &self,
        handle: &RuntimeHandle,
        request: &AgentFileWriteRequest,
    ) -> Result<AgentFileWriteReceipt> {
        self.check(
            handle,
            &request.sandbox_id,
            request.runtime_generation,
            &request.boot_id,
        )?;
        assert!(!request.directory);
        assert_eq!(request.path, FILE_PATH);
        self.writes.fetch_add(1, Ordering::SeqCst);
        let mut stored = self
            .contents
            .as_ref()
            .expect("fixture write support")
            .lock()
            .unwrap();
        let original = format!("{:x}", Sha256::digest(&*stored));
        if request.expected_sha256.as_ref() != Some(&original) {
            return Err(AdapterError::Rejected("file changed".into()));
        }
        *stored = request.content()?;
        if self.lose_write_ack.swap(false, Ordering::SeqCst) {
            return Err(AdapterError::Unavailable(
                "fixture lost write acknowledgement".into(),
            ));
        }
        Ok(AgentFileWriteReceipt {
            path: request.path.clone(),
            size_bytes: stored.len() as u64,
            sha256: format!("{:x}", Sha256::digest(&*stored)),
        })
    }

    fn capabilities(&self) -> AdapterCapabilities {
        panic!("file fixture cannot execute runtime mutations")
    }
    async fn provision(&self, _spec: &SandboxSpec) -> Result<RuntimeHandle> {
        panic!("file fixture cannot execute runtime mutations")
    }
    async fn run_install(
        &self,
        _handle: &RuntimeHandle,
        _request: &InstallRequest,
    ) -> Result<InstallReceipt> {
        panic!("file fixture cannot execute runtime mutations")
    }
    async fn deliver_credential(
        &self,
        _handle: &RuntimeHandle,
        _frame: CredentialFrame,
    ) -> Result<()> {
        panic!("file fixture cannot execute runtime mutations")
    }
    async fn prepare_workspace(
        &self,
        _handle: &RuntimeHandle,
        _request: &WorkspaceRequest,
    ) -> Result<WorkspaceReceipt> {
        panic!("file fixture cannot execute runtime mutations")
    }
    async fn run_workspace_setup(
        &self,
        _handle: &RuntimeHandle,
        _request: &WorkspaceSetupRequest,
    ) -> Result<SetupReceipt> {
        panic!("file fixture cannot execute runtime mutations")
    }
    async fn remove_workspace(
        &self,
        _handle: &RuntimeHandle,
        _request: &WorkspaceRemoveRequest,
    ) -> Result<()> {
        panic!("file fixture cannot execute runtime mutations")
    }
    async fn add_worktree(
        &self,
        _handle: &RuntimeHandle,
        _authority_fence: u64,
        _request: &WorktreeRequest,
    ) -> Result<WorktreeReceipt> {
        panic!("file fixture cannot execute runtime mutations")
    }
    async fn remove_worktree(
        &self,
        _handle: &RuntimeHandle,
        _authority_fence: u64,
        _request: &WorktreeRef,
    ) -> Result<()> {
        panic!("file fixture cannot execute runtime mutations")
    }
    async fn materialize_agent_home(
        &self,
        _handle: &RuntimeHandle,
        _authority_fence: u64,
        _request: &AgentHomeRequest,
    ) -> Result<()> {
        panic!("file fixture cannot execute runtime mutations")
    }
    async fn remove_agent_home(
        &self,
        _handle: &RuntimeHandle,
        _authority_fence: u64,
        _request: &AgentHomeRemoveRequest,
    ) -> Result<()> {
        panic!("file fixture cannot execute runtime mutations")
    }
    async fn open_agent(
        &self,
        _handle: &RuntimeHandle,
        _authority_fence: u64,
        _request: &AgentOpenRequest,
        _updates: mpsc::Sender<ConversationUpdate>,
    ) -> Result<ConversationChannel> {
        panic!("file fixture cannot execute runtime mutations")
    }
    async fn reconnect_agent(
        &self,
        _handle: &RuntimeHandle,
        _authority_fence: u64,
        _request: &AgentOpenRequest,
        _conversation_id: &str,
        _updates: mpsc::Sender<ConversationUpdate>,
    ) -> Result<ConversationChannel> {
        panic!("file fixture cannot execute runtime mutations")
    }
    async fn close_agent(
        &self,
        _handle: &RuntimeHandle,
        _authority_fence: u64,
        _request: &AgentCloseRequest,
    ) -> Result<()> {
        panic!("file fixture cannot execute runtime mutations")
    }
    async fn capture_agent(
        &self,
        _handle: &RuntimeHandle,
        _agent: &AgentSlot,
    ) -> Result<ConversationSnapshot> {
        panic!("file fixture cannot execute runtime mutations")
    }
    async fn checkpoint(
        &self,
        _handle: &RuntimeHandle,
        _request: &CheckpointRequest,
    ) -> Result<CheckpointReceipt> {
        panic!("file fixture cannot execute runtime mutations")
    }
    async fn restore(&self, _handle: &RuntimeHandle, _request: &RestoreRequest) -> Result<()> {
        panic!("file fixture cannot execute runtime mutations")
    }
    async fn resources(&self, _handle: &RuntimeHandle) -> Result<ResourceReading> {
        panic!("file fixture cannot execute runtime mutations")
    }
    async fn teardown(&self, _handle: &RuntimeHandle) -> Result<()> {
        panic!("file fixture cannot execute runtime mutations")
    }
}
