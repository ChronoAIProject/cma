use super::*;
use layer2_core::sandbox::binding::{BindingPolicy, BindingResources, BindingService};

pub(crate) struct BindingComposition {
    service: Arc<BindingService>,
}
impl BindingComposition {
    pub(super) fn new(
        stores: &MongoStores,
        resources: BindingResources,
        policy: Arc<LiveSandboxPolicy>,
        bounds: layer2_core::profiles::ProfileCapacityBounds,
    ) -> Self {
        Self {
            service: BindingService::new(
                stores.sandbox_model.clone(),
                stores.sandbox_profiles.clone(),
                resources,
                crate::store::MongoBindingCredentials::new(
                    stores.sandbox_model.clone(),
                    stores.sandbox_credentials.clone(),
                ),
                BindingPolicy {
                    provider: Default::default(),
                    supported_model_api: "responses".into(),
                    bounds,
                    sandbox: policy,
                },
            ),
        }
    }
    pub(super) fn router(&self) -> Router {
        rest::binding_router(self.service.clone())
    }
    pub(crate) fn spawn(
        self,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(1));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                if *shutdown.borrow() {
                    break;
                }
                tokio::select! {
                    _ = crate::shutdown::signalled(shutdown.clone()) => break,
                    _ = ticker.tick() => {}
                }
                tokio::select! {
                    _ = crate::shutdown::signalled(shutdown.clone()) => break,
                    result = self.service.reconcile(100) => {
                        if let Err(error) = result { tracing::warn!(%error, "binding discovery will retry"); }
                    }
                }
                let _ = shutdown.borrow_and_update();
            }
        })
    }
}

pub(super) fn resource_access(
    stores: &MongoStores,
    auth: Arc<AuthState>,
) -> Arc<layer2_core::resource_access::ResourceAccessService> {
    Arc::new(
        layer2_core::resource_access::ResourceAccessService::new(
            stores.sandbox_model.clone(),
            auth,
        )
        .with_bindings(layer2_core::sandbox::binding::BindingAccessService::new(
            stores.sandbox_model.clone(),
        )),
    )
}

pub(super) fn responses(
    stores: &MongoStores,
    commands: Arc<layer2_core::sandbox::agent_io::AgentCommandService>,
    reads: Arc<layer2_core::sandbox::AgentReadService>,
) -> Router {
    layer1_public::responses::v2::router(
        layer1_public::responses::v2::ResponsesV2State::new(commands, reads).with_bindings(
            layer2_core::sandbox::binding::BindingAccessService::new(stores.sandbox_model.clone()),
        ),
    )
}
