//! Optional routing of Agent control calls made by distributed Sandbox workers.
use crate::{
    wire::{RequestFrame, ResponseFrame},
    RuntimeHandle,
};
use std::sync::{Arc, OnceLock, Weak};

pub type ControlRoutes = Vec<(
    Arc<OwnerControlRoute>,
    Arc<dyn crate::hosted::HostedControlTransport>,
)>;

#[async_trait::async_trait]
pub trait OwnerControlRouter: Send + Sync {
    /// None is reserved for an Agent not yet in durable custody (prepared pools).
    async fn route(
        &self,
        transport: &str,
        handle: &RuntimeHandle,
        request: &RequestFrame,
    ) -> Option<crate::Result<ResponseFrame>>;
}

pub struct OwnerControlRoute {
    pub name: String,
    router: OnceLock<Weak<dyn OwnerControlRouter>>,
}

impl OwnerControlRoute {
    pub fn new(name: &str) -> Arc<Self> {
        Arc::new(Self {
            name: name.into(),
            router: OnceLock::new(),
        })
    }
    pub fn install(&self, router: Arc<dyn OwnerControlRouter>) {
        assert!(
            self.router.set(Arc::downgrade(&router)).is_ok(),
            "control router installed twice"
        );
    }
    pub async fn route(
        &self,
        handle: &RuntimeHandle,
        request: &RequestFrame,
    ) -> Option<crate::Result<ResponseFrame>> {
        if agent_target(request).is_none() || crate::owner_protocol::current_authority().is_some() {
            return None;
        }
        match self.router.get().and_then(Weak::upgrade) {
            Some(router) => router.route(&self.name, handle, request).await,
            None => Some(Err(crate::AdapterError::Unavailable(
                "Agent owner router unavailable".into(),
            ))),
        }
    }
}

pub fn agent_target(request: &RequestFrame) -> Option<&str> {
    Some(match request {
        RequestFrame::PrepareAgentResources(request) => request.request.home.agent_id.as_str(),
        RequestFrame::MaterializeAgentHome(request) => request.request.agent_id.as_str(),
        RequestFrame::RemoveAgentHome(request) => request.request.agent_id.as_str(),
        RequestFrame::InstallAgentSkill(request) => request.request.agent_id.as_str(),
        RequestFrame::OpenAgent(request) => request.request.agent_id.as_str(),
        RequestFrame::CloseAgent(request) => request.request.agent_id.as_str(),
        RequestFrame::CaptureAgent(request) => request.agent_id.as_str(),
        RequestFrame::ProveAgentProcessClosed(request) => request.close.agent_id.as_str(),
        RequestFrame::FenceAgentRestart(request) => request.predecessor.close.agent_id.as_str(),
        RequestFrame::UploadAgentAttachment(request) => request.request.agent_id.as_str(),
        RequestFrame::Browser(request) => request.agent_id.as_str(),
        RequestFrame::BrowserControl(request) => request.agent_id.as_str(),
        _ => return None,
    })
}
