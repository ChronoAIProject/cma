use super::*;
use crate::sandbox::LiveSandboxPolicy;
use axum::Router;
use layer2_core::sandbox::binding::{BindingPolicy, BindingResources, BindingService};

pub(crate) struct BindingComposition {
    service: Arc<BindingService>,
    access: Arc<layer2_core::sandbox::binding::BindingAccessService>,
}
impl BindingComposition {
    pub(super) fn new(
        stores: &MongoStores,
        resources: BindingResources,
        policy: Arc<LiveSandboxPolicy>,
        bounds: layer2_core::profiles::ProfileCapacityBounds,
    ) -> Self {
        Self {
            access: layer2_core::sandbox::binding::BindingAccessService::new(
                stores.sandbox_model.clone(),
            ),
            service: BindingService::new(
                stores.sandbox_model.clone(),
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
    pub(super) fn router(
        &self,
        gateway: Option<&crate::config::GatewayConfig>,
    ) -> Result<Router, String> {
        let routes = match gateway {
            Some(gateway) => rest::binding_router_with_native_tools_aliases_and_preparation(
                self.service.clone(),
                &format!("{}/mcp", gateway.public_url.trim_end_matches('/')),
                &std::iter::once(&gateway.public_api_url)
                    .chain(gateway.public_api_aliases.iter())
                    .map(|base| format!("{}/mcp", base.trim_end_matches('/')))
                    .collect::<Vec<_>>(),
                gateway.binding_preparation_enabled,
            )?,
            None => rest::binding_router(self.service.clone()),
        };
        let routes = routes.merge(rest::binding_conversation_reset_router(
            self.service.clone(),
            gateway.is_some_and(|g| g.conversation_reset_enabled),
        ));
        Ok(
            routes.merge(rest::worker_credentials::worker_credential_router(
                self.access.clone(),
            )),
        )
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
    standing: Arc<dyn layer2_core::account_deletion::AccountStanding>,
) -> Arc<layer2_core::resource_access::ResourceAccessService> {
    Arc::new(
        layer2_core::resource_access::ResourceAccessService::new(
            stores.sandbox_model.clone(),
            auth,
        )
        .with_bindings(layer2_core::sandbox::binding::BindingAccessService::new(
            stores.sandbox_model.clone(),
        ))
        .with_directory(layer2_core::UserDirectory::new(stores.users.clone()))
        .with_account_standing(standing),
    )
}

pub(super) fn responses(
    stores: &MongoStores,
    commands: Arc<layer2_core::sandbox::agent_io::AgentCommandService>,
    reads: Arc<layer2_core::sandbox::AgentReadService>,
    agent_page_origin: layer1_public::agent_page::AgentPageOrigin,
    bindings: &BindingComposition,
    gateway: Option<&crate::config::GatewayConfig>,
    features: crate::config::FeatureFlags,
) -> Result<Router, String> {
    let mut state =
        layer1_public::responses::v2::ResponsesV2State::new(agent_page_origin, commands, reads)
            .with_paged_events(features.paged_response_events)
            .with_bindings(layer2_core::sandbox::binding::BindingAccessService::new(
                stores.sandbox_model.clone(),
            ))
            .with_retained_context_lookup(features.retained_context_lookup);
    if let Some(gateway) = gateway {
        state = state.with_event_router(super::event_routing::CmaEventRouter::new(
            stores.sandbox_model.clone(),
            bindings.service.clone(),
            &gateway.base_url,
        )?);
    }
    Ok(layer1_public::responses::v2::router(state))
}
