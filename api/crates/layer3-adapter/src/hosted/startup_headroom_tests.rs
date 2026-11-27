use super::*;
use crate::codex::{CodexAdapter, CodexSandboxSpec};

fn assignment() -> Arc<Assignment> {
    let request = crate::hosted::HostedDeployment::default()
        .render(&SandboxSpec {
            sandbox_id: "headroom-fixture".into(),
            created_by: Some("fixture".into()),
            ..Default::default()
        })
        .unwrap();
    Arc::new(
        Assignment::for_request(
            &PodBinding {
                sandbox_name: "member".into(),
                sandbox_uid: "member-uid".into(),
                pod_name: "runtime".into(),
                pod_uid: "runtime-uid".into(),
                pod_ip: "127.0.0.1".parse().unwrap(),
                runtime_container_id: Some("container-fixture".into()),
                runtime_restart_count: Some(0),
            },
            &request,
        )
        .unwrap()
        .unwrap(),
    )
}

#[tokio::test]
async fn native_startup_headroom_accounts_for_reused_cache_and_whole_batch() {
    let native = super::super::history_tests::empty_native();
    assert_eq!(native.startup_binding_headroom(&[]), Some(MAX_ASSIGNMENTS));
    let retained = assignment();
    for index in 0..(MAX_ASSIGNMENTS - 9) {
        native.assignments.lock().unwrap().insert(
            RuntimeHandle::new(format!("retained-{index:04}")),
            retained.clone(),
        );
    }
    // Nine bindings fit exactly; eight free entries are insufficient even though
    // the selected concurrent wave would contain only eight operations.
    assert_eq!(native.startup_binding_headroom(&[]), Some(9));
    native
        .assignments
        .lock()
        .unwrap()
        .insert(RuntimeHandle::new("next-retained"), retained.clone());
    assert_eq!(native.startup_binding_headroom(&[]), Some(8));
    for index in 0..8 {
        native.assignments.lock().unwrap().insert(
            RuntimeHandle::new(format!("last-{index}")),
            retained.clone(),
        );
    }
    assert_eq!(native.startup_binding_headroom(&[]), Some(0));
    // Inject the cache's eviction boundary while a status-like snapshot is held.
    // The snapshot alone cannot protect that entry; zero headroom must serialize.
    let handle = native
        .assignments
        .lock()
        .unwrap()
        .first_key_value()
        .unwrap()
        .0
        .clone();
    assert!(native
        .acknowledged_assignment(&handle, &retained.pod, &retained.request)
        .unwrap());
    native.assignments.lock().unwrap().pop_first();
    assert!(!native
        .acknowledged_assignment(&handle, &retained.pod, &retained.request)
        .unwrap());
    assert_eq!(native.startup_binding_headroom(&[]), Some(1));
}

struct Routes;
#[async_trait::async_trait]
impl RuntimeRoutingStore for Routes {
    async fn select(&self, _: &str, _: u64, _: bool) -> Result<bool> {
        panic!("headroom cannot allocate or change a durable route")
    }
}

struct Unknown;
#[async_trait::async_trait]
impl HostedBackend for Unknown {
    async fn provision(&self, _: ProviderSandboxRequest) -> Result<RuntimeHandle> {
        unreachable!()
    }
    async fn exchange(&self, _: &RuntimeHandle, _: RequestFrame) -> Result<ResponseFrame> {
        unreachable!()
    }
    async fn open_agent(
        &self,
        _: &RuntimeHandle,
        _: FencedRequest<AgentLaunch>,
        _: Option<String>,
        _: HostedAgentOpenSettings,
        _: mpsc::Sender<ConversationUpdate>,
    ) -> Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        unreachable!()
    }
    async fn teardown(&self, _: &RuntimeHandle) -> Result<()> {
        unreachable!()
    }
}

#[tokio::test]
async fn startup_headroom_routes_every_selected_provider_through_codex() {
    let native = super::super::history_tests::empty_native();
    let retained = assignment();
    for index in 0..(MAX_ASSIGNMENTS - 9) {
        native.assignments.lock().unwrap().insert(
            RuntimeHandle::new(format!("retained-{index:04}")),
            retained.clone(),
        );
    }
    let native_handle = RuntimeHandle::new("agent-sandbox:member:member-uid");
    let legacy_handle = RuntimeHandle::new("legacy-runtime");
    for unknown in [false, true] {
        let legacy: Arc<dyn HostedBackend> = if unknown {
            Arc::new(Unknown)
        } else {
            Arc::new(crate::hosted::InMemoryHostedBackend::default())
        };
        let adapter = CodexAdapter::with_backend(
            RuntimeBackends {
                agent_sandbox: Some(native.clone()),
                opensandbox: Some(legacy),
                allocate_agent_sandbox: false,
                routing: Arc::new(Routes),
                home_routes: None,
            },
            CodexSandboxSpec::default(),
        );
        assert_eq!(
            adapter.startup_binding_headroom(std::slice::from_ref(&native_handle)),
            Some(9)
        );
        assert_eq!(
            adapter.startup_binding_headroom(std::slice::from_ref(&legacy_handle)),
            if unknown { None } else { Some(usize::MAX) }
        );
        assert_eq!(
            adapter.startup_binding_headroom(&[native_handle.clone(), legacy_handle.clone()]),
            if unknown { None } else { Some(9) }
        );
    }
}
