use super::*;
use layer3_adapter::{
    agent_sandbox::AgentSandboxClient,
    hosted::{HostedAgentOpenSettings, HostedControlTransport},
    wire::{AgentOpenReceipt, FencedReceipt, FencedRequest, ResponseFrame},
    AgentLaunch, ConversationChannel, ConversationUpdate, RuntimeStatus,
};

type Fault = (bool, String, &'static str, serde_json::Value);

pub(super) struct StatusTransport {
    client: AgentSandboxClient,
    state: Arc<Mutex<api::ApiState>>,
    pub entered: tokio::sync::Notify,
    stall: AtomicBool,
    pub fired: AtomicBool,
    pub responses: AtomicUsize,
    pub fault: Mutex<Option<Fault>>,
}
impl StatusTransport {
    pub fn new(client: AgentSandboxClient, state: Arc<Mutex<api::ApiState>>) -> Self {
        Self {
            client,
            state,
            entered: Default::default(),
            stall: AtomicBool::new(true),
            fired: AtomicBool::new(false),
            responses: AtomicUsize::new(0),
            fault: Mutex::new(None),
        }
    }
    fn boundary(&self, after: bool) {
        let fault = self.fault.lock().unwrap();
        if let Some((point, path, pointer, value)) = fault.as_ref() {
            if *point == after && !self.fired.swap(true, Ordering::SeqCst) && !value.is_null() {
                *self
                    .state
                    .lock()
                    .unwrap()
                    .rows
                    .get_mut(path)
                    .unwrap()
                    .pointer_mut(pointer)
                    .unwrap() = value.clone();
            }
        }
    }
}

#[async_trait::async_trait]
impl HostedControlTransport for StatusTransport {
    async fn bind_runtime(&self, _: &RuntimeHandle, _: &str) -> layer3_adapter::Result<()> {
        Ok(())
    }
    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> layer3_adapter::Result<ResponseFrame> {
        let RequestFrame::RuntimeStatus(request) = request else {
            panic!("only status is authorized")
        };
        if self.stall.swap(false, Ordering::SeqCst) {
            self.entered.notify_one();
            std::future::pending::<()>().await;
        }
        self.boundary(false);
        self.client.pod_binding(handle).await?;
        // Capture a reply from the original dispatched request. A subsequent
        // mutation must be refused by the production backend/adapter checks.
        let response = ResponseFrame::RuntimeStatus(RuntimeStatus {
            sandbox_id: request.sandbox_id,
            generation: request.generation,
            home_identity: request.home_identity,
            boot_id: "a".repeat(32),
            installation_ready: true,
            credential_ready: true,
            restore_pending: false,
            agent_processes: 1,
            agent_filesystem: false,
            agent_attachment_upload: false,
            agent_skill_install: true,
            agent_skill_upgrade: false,
            agent_model_catalog: true,
            agent_browser: false,
            agent_resource_preparation: false,
            agent_process_restart: false,
            exact_github_credential_route: false,
            github_repository_scope: false,
        });
        self.responses.fetch_add(1, Ordering::SeqCst);
        self.boundary(true);
        Ok(response)
    }
    async fn open_agent(
        &self,
        _: &RuntimeHandle,
        _: FencedRequest<AgentLaunch>,
        _: Option<String>,
        _: HostedAgentOpenSettings,
        _: tokio::sync::mpsc::Sender<ConversationUpdate>,
    ) -> layer3_adapter::Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        panic!("reconciliation must not reopen a child")
    }
    async fn retire(&self, _: &RuntimeHandle) -> layer3_adapter::Result<()> {
        panic!("held runtime must not be retired")
    }
}
