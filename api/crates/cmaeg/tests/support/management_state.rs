use cmaeg::{
    auth::CreatorAuthenticator,
    channels::management::ManagementState,
    providers::{ProviderClient, ProviderRegistry},
    records::RecordClient,
};
use cmaeg_core::seal::MasterKeyRing;
use std::sync::Arc;

pub fn state(
    registry: ProviderRegistry,
    records: Arc<RecordClient>,
    master_keys: Arc<MasterKeyRing>,
    cma_base_url: &str,
    nyxid_base_url: &str,
) -> ManagementState {
    let providers = Arc::new(
        ProviderClient::new(
            registry,
            std::time::Duration::from_secs(1),
            std::time::Duration::from_secs(2),
            8,
        )
        .unwrap(),
    );
    ManagementState::new(
        providers,
        records,
        Arc::new(CreatorAuthenticator::new(nyxid_base_url).unwrap()),
        cmaeg::ManagementConfig {
            console_public_url: None,
            master_keys,
            public_url: "http://127.0.0.1:8090".into(),
            public_api_url: "http://127.0.0.1:8090".into(),
            public_api_aliases: vec![],
            advertise_public_api_url: true,
            cma_base_url: cma_base_url.into(),
            tools_nyxid_slug: "cmaeg".into(),
            cma_service_token: cmaeg_core::channel::Secret::new("cma-service"),
            nyxid_base_url: nyxid_base_url.into(),
        },
        cmaeg::ChannelCacheConfig::default(),
    )
}
