//! Public management and independently authenticated callback composition.
use std::sync::Arc;

use axum::Router;
use layer2_core::{
    bridge::{invoke::BridgeInvokeService, BridgeService},
    resource_access::ResourceAccessService,
};

use crate::{
    config::Config,
    store::{MongoBridgeAllocationStore, MongoBridgeInvokeStore, MongoCallbackInbox, MongoStores},
};

pub(crate) struct BridgeComposition {
    pub management: Router,
    pub callbacks: Router,
    pub invokes: BridgeInvokeService,
    pub allocations: Arc<MongoBridgeAllocationStore>,
}

impl BridgeComposition {
    pub(crate) async fn build(
        config: &Config,
        stores: &MongoStores,
        definitions: BridgeService,
        credentials: Option<Arc<super::ManagedBridgeCredentials>>,
        access: Arc<ResourceAccessService>,
    ) -> Result<Self, String> {
        let invoke_store = Arc::new(
            MongoBridgeInvokeStore::open(
                stores.bridge_definitions.clone(),
                stores.sandbox_model.clone(),
                &config.mongodb_database,
            )
            .await
            .map_err(|error| format!("bridge invoke storage is not usable: {error}"))?,
        );
        let allocations = Arc::new(
            MongoBridgeAllocationStore::open(
                stores.bridge_definitions.clone(),
                Some(invoke_store.clone()),
                &config.mongodb_database,
            )
            .await
            .map_err(|error| format!("bridge allocation storage is not usable: {error}"))?,
        );
        let invokes = BridgeInvokeService::new(
            definitions.clone(),
            access,
            stores.sandbox_model.clone(),
            invoke_store,
        );
        let setup = super::setup::service(&config.auth_mode, definitions, credentials.clone())
            .map_err(|error| format!("bridge setup is not usable: {error}"))?
            .map(|setup| {
                Arc::new(
                    (*setup)
                        .clone()
                        .with_invoke_grants(Arc::new(invokes.clone())),
                )
            });
        let management = layer1_public::rest::bridge_router(setup).merge(
            layer1_public::rest::bridge_invoke_router(Some(Arc::new(invokes.clone()))),
        );
        let inbox = Arc::new(
            MongoCallbackInbox::open(
                &stores.bridge_definitions,
                &config.mongodb_database,
                &config.deployment_id,
                &config.auth_encryption_key,
            )
            .await
            .map_err(|error| format!("bridge callback inbox is not usable: {error}"))?,
        );
        let ingress = super::ingress::configured(
            &config.auth_mode,
            config.nyxid_public_url.as_deref(),
            stores.bridge_definitions.clone(),
            credentials,
            stores.sandbox_model.clone(),
            inbox,
        )
        .map_err(|error| format!("bridge callback verifier is not usable: {error}"))?;
        Ok(Self {
            management,
            callbacks: super::ingress::http::router(ingress),
            invokes,
            allocations,
        })
    }
}
