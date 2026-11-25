use super::*;
use crate::wire::{AgentOpenReceipt, FencedReceipt, FencedRequest};
use crate::{AgentLaunch, ConversationChannel, ConversationUpdate, RuntimeObservation};
use std::sync::Mutex;

pub(in crate::hosted) struct Files {
    text: Mutex<String>,
    pub(in crate::hosted) status_reads: std::sync::atomic::AtomicUsize,
    pub(in crate::hosted) writes: std::sync::atomic::AtomicUsize,
    pub(in crate::hosted) goal_probes: std::sync::atomic::AtomicUsize,
    fault: u8,
}
#[async_trait::async_trait]
impl HostedBackend for Files {
    async fn run_recovery_command(
        &self,
        handle: &RuntimeHandle,
        command: &crate::hosted::RuntimeRecoveryCommand,
    ) -> crate::Result<Vec<u8>> {
        self.goal_probes
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(handle.0, "retained-runtime");
        assert_eq!(command.program, "/usr/bin/python3");
        assert_eq!(&command.args[..2], ["-I", "-c"]);
        let request: serde_json::Value = serde_json::from_slice(&command.input).unwrap();
        assert_eq!(request["home"], "/home/cma/.cma/agents/agent/home");
        if self.fault == 9 {
            return Err(AdapterError::Unavailable("probe unavailable".into()));
        }
        if self.fault == 8 {
            return Ok(b"unknown output".to_vec());
        }
        Ok(serde_json::to_vec(&serde_json::json!({
            "version":1,
            "session":if self.fault == 7 { serde_json::json!("other") } else { request["session"].clone() },
            "goal_absent":self.fault != 6,
        })).unwrap())
    }
    async fn provision(
        &self,
        _: crate::provider::ProviderSandboxRequest,
    ) -> crate::Result<RuntimeHandle> {
        unreachable!()
    }
    async fn teardown(&self, _: &RuntimeHandle) -> crate::Result<()> {
        unreachable!()
    }
    async fn inventory(&self) -> crate::Result<Vec<RuntimeObservation>> {
        unreachable!()
    }
    async fn open_agent(
        &self,
        _: &RuntimeHandle,
        _: FencedRequest<AgentLaunch>,
        _: Option<String>,
        _: super::super::HostedAgentOpenSettings,
        _: tokio::sync::mpsc::Sender<ConversationUpdate>,
    ) -> crate::Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        unreachable!()
    }
    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> crate::Result<ResponseFrame> {
        assert_eq!(handle.0, "retained-runtime");
        let mut text = self.text.lock().unwrap();
        match request {
            RequestFrame::RuntimeStatus(identity) => {
                let reads = self
                    .status_reads
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(ResponseFrame::RuntimeStatus(RuntimeStatus {
                    agent_filesystem: true,
                    agent_attachment_upload: false,
                    agent_skill_install: false,
                    agent_skill_upgrade: false,
                    agent_model_catalog: false,
                    agent_browser: false,
                    agent_resource_preparation: false,
                    agent_process_restart: false,
                    exact_github_credential_route: false,
                    github_repository_scope: false,
                    sandbox_id: identity.sandbox_id,
                    generation: identity.generation + u64::from(self.fault == 5),
                    home_identity: identity.home_identity,
                    boot_id: if self.fault == 4 && reads > 0 {
                        "b".repeat(32)
                    } else {
                        "a".repeat(32)
                    },
                    installation_ready: true,
                    credential_ready: true,
                    restore_pending: false,
                    agent_processes: 1,
                }))
            }
            RequestFrame::ReadAgentFile(read) => {
                assert!(read.owner_access);
                assert!(read.protected_paths.is_empty());
                assert_eq!(read.sandbox_id, "sandbox");
                assert_eq!(read.runtime_generation, 3);
                assert_eq!(read.boot_id, "a".repeat(32));
                assert_eq!(read.path, "cma/.cma/agents/agent/home/AGENTS.md");
                Ok(ResponseFrame::ReadAgentFile(crate::AgentFileReadReceipt {
                    path: read.path,
                    size_bytes: text.len() as u64,
                    sha256: format!("{:x}", Sha256::digest(text.as_bytes())),
                    content_base64: base64::engine::general_purpose::STANDARD
                        .encode(text.as_bytes()),
                }))
            }
            RequestFrame::WriteAgentFile(write) => {
                self.writes
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                assert!(write.owner_access);
                assert!(!write.directory);
                assert_eq!(write.path, "cma/.cma/agents/agent/home/AGENTS.md");
                assert_eq!(write.runtime_generation, 3);
                assert_eq!(write.boot_id, "a".repeat(32));
                if self.fault == 1 {
                    *text = "Concurrent owner edit".into();
                }
                if write.expected_sha256.as_deref()
                    != Some(format!("{:x}", Sha256::digest(text.as_bytes())).as_str())
                {
                    return Err(AdapterError::Rejected("file changed concurrently".into()));
                }
                *text = String::from_utf8(write.content()?).unwrap();
                if self.fault == 2 {
                    return Err(AdapterError::Unavailable("write ACK lost".into()));
                }
                Ok(ResponseFrame::WriteAgentFile(
                    crate::AgentFileWriteReceipt {
                        path: write.path,
                        size_bytes: text.len() as u64,
                        sha256: if self.fault == 3 {
                            "b".repeat(64)
                        } else {
                            format!("{:x}", Sha256::digest(text.as_bytes()))
                        },
                    },
                ))
            }
            _ => unreachable!(),
        }
    }
}
pub(in crate::hosted) fn binding(fault: u8) -> (HostedContextRefresh, Arc<Files>) {
    binding_with_tools(fault, Vec::new())
}

pub(in crate::hosted) fn binding_with_tools(
    fault: u8,
    tools: Vec<serde_json::Value>,
) -> (HostedContextRefresh, Arc<Files>) {
    let files = Arc::new(Files {
        text: Mutex::new("Old managed\n\nOwner exact text  \n".into()),
        status_reads: std::sync::atomic::AtomicUsize::new(0),
        writes: std::sync::atomic::AtomicUsize::new(0),
        goal_probes: std::sync::atomic::AtomicUsize::new(0),
        fault,
    });
    let binding = HostedContextRefresh::new(
        files.clone(),
        RuntimeHandle("retained-runtime".into()),
        RuntimeStatusRequest {
            sandbox_id: "sandbox".into(),
            generation: 3,
            home_identity: "a".repeat(43),
        },
        "/home/cma/.cma/agents/agent/home",
        "/home/cma/work".into(),
        tools,
    )
    .unwrap();
    (binding, files)
}
pub(in crate::hosted) fn owned_binding(
    policy: &crate::ingress_budget::IngressOwnershipPolicy,
    fault: u8,
) -> (
    HostedContextRefresh,
    crate::ingress_budget::SharedOwned<Files>,
) {
    owned_binding_with_tools(policy, fault, &[])
}

pub(in crate::hosted) fn owned_binding_with_tools(
    policy: &crate::ingress_budget::IngressOwnershipPolicy,
    fault: u8,
    tools: &[serde_json::Value],
) -> (
    HostedContextRefresh,
    crate::ingress_budget::SharedOwned<Files>,
) {
    use super::binding::{ContextInput, ContextSources};
    let original = "Old managed\n\nOwner exact text  \n";
    let files = super::binding_fixture::source(
        policy,
        original.len() + super::binding_fixture::mutex_backing(),
        || Files {
            text: Mutex::new(original.into()),
            status_reads: std::sync::atomic::AtomicUsize::new(0),
            writes: std::sync::atomic::AtomicUsize::new(0),
            goal_probes: std::sync::atomic::AtomicUsize::new(0),
            fault,
        },
    );
    drop(files.text.lock().unwrap());
    let binding = HostedContextRefresh::new_admitted(
        policy,
        ContextSources {
            backend: files.clone().into(),
            access: None,
            provider: None,
        },
        ContextInput {
            handle: &RuntimeHandle("retained-runtime".into()),
            identity: &RuntimeStatusRequest {
                sandbox_id: "sandbox".into(),
                generation: 3,
                home_identity: "a".repeat(43),
            },
            home: "/home/cma/.cma/agents/agent/home",
            cwd: "/home/cma/work",
            tools,
        },
    )
    .unwrap();
    (binding, files)
}

#[tokio::test]
async fn exact_home_write_uses_compare_and_swap_and_confirms_receipt() {
    let (binding, files) = binding(0);
    binding
        .fixture_append_guidance("New managed", &["Old managed".into()])
        .await
        .unwrap();
    assert_eq!(
        *files.text.lock().unwrap(),
        "New managed\n\nOwner exact text  \n"
    );
    binding
        .fixture_append_guidance("New managed", &["Old managed".into()])
        .await
        .unwrap();
    assert_eq!(files.writes.load(std::sync::atomic::Ordering::SeqCst), 1);
}
#[tokio::test]
async fn concurrent_owner_file_edit_wins_over_managed_refresh() {
    let (binding, files) = binding(1);
    assert!(binding
        .fixture_append_guidance("New managed", &["Old managed".into()])
        .await
        .is_err());
    assert_eq!(*files.text.lock().unwrap(), "Concurrent owner edit");
}
#[tokio::test]
async fn lost_write_ack_replays_without_duplicate_and_corrupt_receipt_is_refused() {
    for fault in [2, 3] {
        let (binding, files) = binding(fault);
        assert!(binding
            .fixture_append_guidance("New managed", &["Old managed".into()])
            .await
            .is_err());
        binding
            .fixture_append_guidance("New managed", &["Old managed".into()])
            .await
            .unwrap();
        assert_eq!(
            *files.text.lock().unwrap(),
            "New managed\n\nOwner exact text  \n"
        );
        assert_eq!(files.writes.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}

impl HostedContextRefresh {
    async fn fixture_append_guidance(
        &self,
        guidance: &str,
        previous: &[String],
    ) -> crate::Result<()> {
        let receipt = self.readiness_receipt("retained-session", 1).await?;
        self.append_guidance("retained-session", 1, &receipt, guidance, previous)
            .await
    }
}

#[tokio::test]
async fn stale_runtime_readiness_receipt_never_writes_a_replacement_boot() {
    let (binding, files) = binding(4);
    let receipt = binding
        .readiness_receipt("retained-session", 1)
        .await
        .unwrap();
    assert!(matches!(
        binding
            .append_guidance(
                "retained-session",
                1,
                &receipt,
                "New managed",
                &["Old managed".into()]
            )
            .await,
        Err(AdapterError::OperationIdentityConflict(_))
    ));
    assert_eq!(files.writes.load(std::sync::atomic::Ordering::SeqCst), 0);
}
