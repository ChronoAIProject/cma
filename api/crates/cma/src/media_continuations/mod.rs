//! Exact Gateway media artifacts delivered to an already admitted Agent event.
mod model;
mod parent;
pub(crate) mod protocol;
mod source;
mod supplement;
mod upload;
use layer2_core::{
    resource_access::ResourceAccessService,
    sandbox::{
        agent_attachments::AgentAttachmentService,
        agent_io::{AgentCommandAuthorizer, AgentCommandService, AgentCommandStore},
    },
};
pub(crate) use model::{MediaUploadRecord, MediaUploadStore};
use std::sync::Arc;
pub(crate) struct MediaContinuations {
    ownership: std::sync::OnceLock<Arc<layer2_core::agent_ownership::OwnershipCoordinator>>,
    commands: Arc<AgentCommandService>,
    store: Arc<dyn AgentCommandStore>,
    uploads: Arc<dyn MediaUploadStore>,
    access: Arc<ResourceAccessService>,
    authority: Arc<dyn AgentCommandAuthorizer>,
    attachments: Arc<AgentAttachmentService>,
    client: reqwest::Client,
    gateway: url::Url,
}
impl MediaContinuations {
    pub(crate) fn install_ownership(
        &self,
        ownership: Arc<layer2_core::agent_ownership::OwnershipCoordinator>,
    ) -> Result<(), String> {
        self.ownership
            .set(ownership)
            .map_err(|_| "media ownership already installed".into())
    }

    pub(crate) fn new(
        commands: Arc<AgentCommandService>,
        store: Arc<dyn AgentCommandStore>,
        uploads: Arc<dyn MediaUploadStore>,
        access: Arc<ResourceAccessService>,
        authority: Arc<dyn AgentCommandAuthorizer>,
        attachments: Arc<AgentAttachmentService>,
        gateway: &str,
    ) -> Result<Arc<Self>, String> {
        let mut gateway = url::Url::parse(gateway).map_err(|_| "invalid Gateway origin")?;
        if !matches!(gateway.scheme(), "http" | "https")
            || !gateway.username().is_empty()
            || gateway.password().is_some()
            || gateway.query().is_some()
            || gateway.fragment().is_some()
        {
            return Err("invalid Gateway origin".into());
        }
        gateway.set_path("/v1/event-attestations");
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .map_err(|_| "Gateway client unavailable")?;
        Ok(Arc::new(Self {
            ownership: std::sync::OnceLock::new(),
            commands,
            store,
            uploads,
            access,
            authority,
            attachments,
            client,
            gateway,
        }))
    }
}
