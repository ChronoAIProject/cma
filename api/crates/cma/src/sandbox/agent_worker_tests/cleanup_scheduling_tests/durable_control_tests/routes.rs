use super::*;
use crate::agent_manager::relay::{PublicAccess, Relay};
use crate::auth::{AuthMode, AuthState};

pub(super) fn compose(
    coordinator: Arc<OwnershipCoordinator>,
    updates: &Arc<DurableAgentUpdateRecorder>,
    routes: Router,
    store: Arc<MongoSandboxStore>,
    adapter: Arc<dyn SandboxAdapter>,
    identity: Option<&str>,
) -> Router {
    let mode = match identity {
        Some(subject) => AuthMode::Trusted {
            subject: subject.into(),
        },
        None => AuthMode::Provider {
            base_url: "http://127.0.0.1:1".into(),
            client_id: "fixture".into(),
            client_secret: "fixture".into(),
            public_url: "http://localhost".into(),
        },
    };
    let auth = AuthState::in_memory(mode, None);
    let access = Arc::new(layer2_core::resource_access::ResourceAccessService::new(
        store.clone(),
        auth.clone(),
    ));
    let reads =
        AgentReadService::new(store, Arc::new(Resources)).with_resource_access(access.clone());
    let relay = Relay::new(
        coordinator,
        updates,
        ring::hmac::Key::new(ring::hmac::HMAC_SHA256, b"durable-control-fixture"),
    )
    .unwrap();
    relay
        .install_public_access(PublicAccess::new(auth.clone(), reads, access, adapter))
        .unwrap();
    relay.authenticated_control_routes(routes, auth, None)
}
