//! Mount actual personal recovery commands under the normal authenticated router.
use std::sync::Arc;

use axum::Router;
use layer2_core::{
    resource_access::ResourceAccessService,
    sandbox::agent_capacity_recovery::service::CapacityRecoveryFeatures,
};

use crate::{
    sandbox::SandboxLifecycleComposition,
    store::{agent_capacity_recovery::RecoveryService, MongoStores},
};

pub(super) fn router(
    stores: &MongoStores,
    access: Arc<ResourceAccessService>,
    lifecycle: &mut SandboxLifecycleComposition,
    admission_enabled: bool,
) -> Router {
    let service = Arc::new(
        RecoveryService::new(stores.sandbox_model.as_ref().clone(), access).with_features(
            CapacityRecoveryFeatures {
                enabled: admission_enabled,
            },
        ),
    );
    // Mount accepted-work/status paths even when new admission is closed.
    // This retains fresh personal authorization on every command; discovery
    // metadata is never promoted into a Caller or a background execution grant.
    layer1_public::rest::agent_capacity_recovery_commands_router(
        lifecycle.capacity_recovery_commands(stores.sandbox_model.clone(), service),
    )
}
