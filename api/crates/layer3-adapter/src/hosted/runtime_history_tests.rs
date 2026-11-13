//! The production Codex -> routing -> provider -> transport chain must export history.
use super::*;
use crate::agent_sandbox::AgentSandboxConfig;
use crate::codex::{CodexAdapter, CodexSandboxSpec};
use crate::hosted::ProviderHostedBackend;
use crate::provider::SandboxClient;
use std::sync::Mutex;

struct ExistingRuntimeRoutes;
#[async_trait::async_trait]
impl RuntimeRoutingStore for ExistingRuntimeRoutes {
    async fn select(&self, _: &str, _: u64, _: bool) -> Result<bool> {
        panic!("history must follow the existing handle, not choose an allocation provider");
    }
}

struct HistoryTransport {
    bytes: &'static [u8],
    calls: Mutex<Vec<(RuntimeHandle, AgentSlot)>>,
}
#[async_trait::async_trait]
impl HostedControlTransport for HistoryTransport {
    async fn exchange(&self, _: &RuntimeHandle, _: RequestFrame) -> Result<ResponseFrame> {
        panic!("history must not issue a control operation");
    }
    async fn open_agent(
        &self,
        _: &RuntimeHandle,
        _: FencedRequest<AgentLaunch>,
        _: Option<String>,
        _: HostedAgentOpenSettings,
        _: mpsc::Sender<ConversationUpdate>,
    ) -> Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        panic!("history must not open an Agent");
    }
    async fn retire(&self, _: &RuntimeHandle) -> Result<()> {
        panic!("history must not retire a runtime");
    }
    async fn export_agent_history(
        &self,
        handle: &RuntimeHandle,
        agent: &AgentSlot,
    ) -> Result<Vec<u8>> {
        self.calls
            .lock()
            .unwrap()
            .push((handle.clone(), agent.clone()));
        if agent.as_str() == "denied" {
            return Err(AdapterError::Rejected("history access refused".into()));
        }
        Ok(self.bytes.to_vec())
    }
}

fn native(transport: Arc<dyn HostedControlTransport>) -> Arc<dyn HostedBackend> {
    let client = AgentSandboxClient::new(
        kube::Client::try_from(kube::Config::new("http://127.0.0.1:1".parse().unwrap())).unwrap(),
        AgentSandboxConfig {
            namespace: "cma-test".into(),
            deployment: "fixture".into(),
            runtime_class: None,
            service_account: "cma-runtime".into(),
            image_pull_secrets: vec![],
            template_network_policy: None,
            template_fs_group_change_policy: None,
            native_claim_guard_enabled: false,
            control_key: "fixture".repeat(8),
        },
        "fixture",
    )
    .unwrap();
    Arc::new(AgentSandboxBackend::new(client, transport))
}

fn legacy(transport: Arc<dyn HostedControlTransport>) -> Arc<dyn HostedBackend> {
    Arc::new(
        ProviderHostedBackend::new(
            SandboxClient::new("http://127.0.0.1:1", "fixture"),
            transport,
            "fixture",
        )
        .unwrap(),
    )
}

#[tokio::test]
async fn history_follows_existing_handles_through_both_production_backends() {
    let native_history = Arc::new(HistoryTransport {
        bytes: b"{\"provider\":\"native\"}\n",
        calls: Mutex::new(vec![]),
    });
    let legacy_history = Arc::new(HistoryTransport {
        bytes: b"{\"provider\":\"legacy\"}\n",
        calls: Mutex::new(vec![]),
    });
    let native_handle = RuntimeHandle::new("agent-sandbox:runtime-one:original-uid");
    let legacy_handle = RuntimeHandle::new("legacy-runtime");
    let agent = AgentSlot::new("agt_history");
    for default_new in [false, true] {
        let adapter = CodexAdapter::with_backend(
            RuntimeBackends {
                home_routes: None,
                agent_sandbox: Some(native(native_history.clone())),
                opensandbox: Some(legacy(legacy_history.clone())),
                allocate_agent_sandbox: default_new,
                routing: Arc::new(ExistingRuntimeRoutes),
            },
            CodexSandboxSpec::default(),
        );
        assert_eq!(
            adapter
                .export_agent_history(&native_handle, &agent)
                .await
                .unwrap(),
            native_history.bytes
        );
        assert_eq!(
            adapter
                .export_agent_history(&legacy_handle, &agent)
                .await
                .unwrap(),
            legacy_history.bytes
        );
        assert!(
            matches!(adapter.export_agent_history(&native_handle, &AgentSlot::new("denied")).await,
            Err(AdapterError::Rejected(detail)) if detail == "history access refused")
        );
    }
    assert_eq!(
        *legacy_history.calls.lock().unwrap(),
        vec![(legacy_handle, agent.clone()); 2]
    );
    assert_eq!(
        *native_history.calls.lock().unwrap(),
        vec![
            (native_handle.clone(), agent.clone()),
            (native_handle.clone(), AgentSlot::new("denied")),
            (native_handle.clone(), agent),
            (native_handle, AgentSlot::new("denied")),
        ]
    );
}

#[tokio::test]
async fn unavailable_history_provider_does_not_fall_back_to_another_provider() {
    let transport = Arc::new(HistoryTransport {
        bytes: b"unrelated",
        calls: Mutex::new(vec![]),
    });
    for native_missing in [true, false] {
        let backend = RuntimeBackends {
            home_routes: None,
            agent_sandbox: (!native_missing).then(|| native(transport.clone())),
            opensandbox: native_missing.then(|| legacy(transport.clone())),
            allocate_agent_sandbox: !native_missing,
            routing: Arc::new(ExistingRuntimeRoutes),
        };
        let handle = RuntimeHandle::new(if native_missing {
            "agent-sandbox:runtime-one:uid"
        } else {
            "legacy-runtime"
        });
        assert!(matches!(
            backend
                .export_agent_history(&handle, &AgentSlot::new("agt_history"))
                .await,
            Err(AdapterError::Unavailable(_))
        ));
    }
    assert!(transport.calls.lock().unwrap().is_empty());
}
