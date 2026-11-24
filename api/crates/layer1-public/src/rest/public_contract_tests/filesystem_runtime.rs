//! Deterministic runtime boundary for real filesystem REST/core replay.
use base64::Engine as _;
use layer2_core::{
    sandbox::{SandboxHome, SandboxNativePool},
    SandboxId,
};
use layer3_adapter::*;
use sha2::{Digest as _, Sha256};
use std::{collections::BTreeMap, sync::Mutex};
use tokio::sync::mpsc;

pub(super) struct Files {
    pub kind: String,
    pub entries: Mutex<BTreeMap<String, Option<Vec<u8>>>>,
    pub browser_reply: Mutex<BrowserObservation>,
    pub browser_calls: std::sync::atomic::AtomicUsize,
    pub browser_barrier: Mutex<Option<std::sync::Arc<tokio::sync::Barrier>>>,
    pub control_receipts: Mutex<BTreeMap<String, (BrowserControlRequest, BrowserControlReceipt)>>,
    pub root: Option<std::path::PathBuf>,
    pub browser_relay: Option<std::path::PathBuf>,
}
impl Files {
    pub fn new(kind: &str) -> Self {
        Self {
            kind: kind.into(),
            browser_reply: Mutex::new(BrowserObservation::idle(BrowserState::NotEnabled)),
            browser_calls: Default::default(),
            browser_barrier: Mutex::new(None),
            control_receipts: Mutex::new(BTreeMap::new()),
            root: None,
            browser_relay: None,
            entries: Mutex::new(BTreeMap::from([
                ("cma".into(), None),
                (
                    "cma/hello world.txt".into(),
                    Some(b"hello files\n".to_vec()),
                ),
                ("cma/.hidden".into(), Some(b"hidden\n".to_vec())),
            ])),
        }
    }
    fn check(
        &self,
        handle: &RuntimeHandle,
        sandbox: &str,
        generation: u64,
        boot: &str,
        path: &str,
        owner: bool,
    ) -> Result<()> {
        assert_eq!(handle.as_str(), "runtime-route");
        assert_eq!(sandbox, "sbx_route");
        assert_eq!(generation, 1);
        assert_eq!(boot, "b".repeat(32));
        assert_eq!(owner, !matches!(self.kind.as_str(), "viewer" | "writer"));
        assert!(
            valid_agent_file_path(path),
            "invalid path crossed the core boundary"
        );
        if self.kind == "runtime_capacity" {
            return Err(
                crate::rest::public_contract_tests::runtime_capacity_cases::request_failure(),
            );
        }
        if self.kind == "unavailable" {
            return Err(AdapterError::Unavailable(
                "fixture runtime unavailable".into(),
            ));
        }
        if self.kind == "too_large" {
            return Err(AdapterError::Rejected(
                "file_too_large: bounded response".into(),
            ));
        }
        if self.kind == "runtime_unsupported" {
            return Err(AdapterError::Unsupported("fixture unsupported".into()));
        }
        if self.kind == "runtime_conflict" {
            return Err(AdapterError::Rejected("file changed".into()));
        }
        Ok(())
    }
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn missing() -> AdapterError {
    AdapterError::Rejected("file_not_found: absent".into())
}

#[async_trait::async_trait]
impl SandboxAdapter for Files {
    fn browser(&self) -> Option<&dyn BrowserAdapter> {
        self.kind.starts_with("browser").then_some(self)
    }
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
            &request.path,
            request.owner_access,
        )?;
        let files = self.entries.lock().unwrap();
        if !request.path.is_empty() && files.get(&request.path) != Some(&None) {
            return Err(missing());
        }
        let prefix = if request.path.is_empty() {
            String::new()
        } else {
            format!("{}/", request.path)
        };
        let entries = files
            .iter()
            .filter_map(|(path, bytes)| {
                let name = path.strip_prefix(&prefix)?;
                if name.is_empty() || name.contains('/') {
                    return None;
                }
                Some(AgentFileEntry {
                    path: path.clone(),
                    name: name.into(),
                    kind: if bytes.is_some() {
                        AgentFileKind::File
                    } else {
                        AgentFileKind::Directory
                    },
                    size_bytes: bytes.as_ref().map_or(0, |b| b.len() as u64),
                    sha256: None,
                    hidden: name.starts_with('.'),
                })
            })
            .collect();
        Ok(AgentFileListReceipt {
            path: request.path.clone(),
            entries,
        })
    }
    async fn read_agent_file(
        &self,
        handle: &RuntimeHandle,
        request: &AgentFileReadRequest,
    ) -> Result<AgentFileReadReceipt> {
        self.check(
            handle,
            &request.sandbox_id,
            request.runtime_generation,
            &request.boot_id,
            &request.path,
            request.owner_access,
        )?;
        let files = self.entries.lock().unwrap();
        let bytes = if let Some(root) = &self.root {
            std::fs::read(root.join(&request.path)).map_err(|_| missing())?
        } else {
            files
                .get(&request.path)
                .and_then(Option::as_ref)
                .ok_or_else(missing)?
                .clone()
        };
        if bytes.len() as u64 > request.max_bytes {
            return Err(AdapterError::Rejected(
                "file_too_large: bounded read".into(),
            ));
        }
        Ok(AgentFileReadReceipt {
            path: request.path.clone(),
            size_bytes: bytes.len() as u64,
            sha256: digest(&bytes),
            content_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
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
            &request.path,
            request.owner_access,
        )?;
        if request.path.is_empty() {
            return Err(AdapterError::Rejected("home root is not writable".into()));
        }
        let bytes = request.content()?;
        let mut files = self.entries.lock().unwrap();
        if let Some((parent, _)) = request.path.rsplit_once('/') {
            if files.get(parent) != Some(&None) {
                return Err(missing());
            }
        }
        if let Some(Some(previous)) = files.get(&request.path) {
            if previous != &bytes && request.expected_sha256.as_ref() != Some(&digest(previous)) {
                return Err(AdapterError::Rejected("file changed".into()));
            }
        } else if request.expected_sha256.is_some() {
            return Err(AdapterError::Rejected("file changed".into()));
        }
        files.insert(
            request.path.clone(),
            if request.directory {
                None
            } else {
                Some(bytes.clone())
            },
        );
        Ok(AgentFileWriteReceipt {
            path: request.path.clone(),
            size_bytes: bytes.len() as u64,
            sha256: digest(&bytes),
        })
    }
    async fn delete_agent_file(
        &self,
        handle: &RuntimeHandle,
        request: &AgentFileDeleteRequest,
    ) -> Result<AgentFileDeleteReceipt> {
        self.check(
            handle,
            &request.sandbox_id,
            request.runtime_generation,
            &request.boot_id,
            &request.path,
            request.owner_access,
        )?;
        if request.path.is_empty() {
            return Err(AdapterError::Rejected("home root cannot be deleted".into()));
        }
        let mut files = self.entries.lock().unwrap();
        if !files.contains_key(&request.path) {
            return Err(missing());
        }
        let prefix = format!("{}/", request.path);
        if !request.recursive && files.keys().any(|p| p.starts_with(&prefix)) {
            return Err(AdapterError::Rejected("directory is not empty".into()));
        }
        files.retain(|p, _| p != &request.path && !p.starts_with(&prefix));
        Ok(AgentFileDeleteReceipt {
            path: request.path.clone(),
            deleted: true,
        })
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

pub(super) fn persistent_home() -> SandboxHome {
    use layer3_adapter::agent_sandbox::*;
    let policy = HomeVolumePolicy {
        mode: HomeVolumeMode::NativeVctFullHomeV2,
        storage_class: "standard-rwo".into(),
        capacity_gib: 8,
        template_name: String::new(),
        template_generation: String::new(),
    };
    let mut home = SandboxHome::reserve(&SandboxId("sbx_route".into()), policy.clone()).unwrap();
    home.request.scope = Some(
        HomeVolumeScope::from_authority(
            "native-vct-empty-v1",
            &["attachment-test", "default", "runtime"],
        )
        .unwrap(),
    );
    let request = NativePoolClaimRequest {
        pool: NativePoolBinding {
            spec: NativePoolSpec {
                cold_canary: false,
                image: RuntimeImage {
                    uri: format!("registry.example/runtime@sha256:{}", "a".repeat(64)),
                    labels: std::collections::BTreeMap::from([
                        (RUNTIME_CONTRACT_LABEL.into(), "3".into()),
                        ("dev.chrono.cma.runtime-bootstrap".into(), "1".into()),
                        ("dev.chrono.cma.home-layout".into(), "2".into()),
                    ]),
                },
                cpu_millis: 1000,
                memory_mib: 2048,
                home: policy,
            },
            pod_policy: NativePoolPodPolicy {
                runtime_class: None,
                service_account: "runtime".into(),
                image_pull_secrets: vec![],
                template_network_policy: None,
                fs_group_change_policy: None,
            },
            name: "pool".into(),
            uid: "pool-uid".into(),
            template_name: "template".into(),
            template_uid: "template-uid".into(),
            config_sha256: "a".repeat(64),
        },
        home: home.request.clone(),
        runtime_generation: 1,
    };
    let claim = NativePoolClaim {
        request: request.clone(),
        name: "claim".into(),
        uid: "claim-uid".into(),
        resource_version: "1".into(),
        sandbox_name: Some("member".into()),
    };
    home.native_pool = Some(Box::new(SandboxNativePool {
        request,
        claim: Some(claim.clone()),
        cleanup: None,
        cleaned: false,
    }));
    home.bind(HomeVolumeReceipt {
        native_authority: Some(Box::new(NativePoolHomeOrigin {
            claim,
            sandbox_uid: "sandbox-uid".into(),
            pvc_uid: "pvc-uid".into(),
        })),
        volume_id: home.request.volume_id.clone(),
        request_sha256: home.request.computed_sha256(),
        storage_reference: "home-member".into(),
        storage_generation: "pvc-uid".into(),
    })
    .unwrap();
    home
}
