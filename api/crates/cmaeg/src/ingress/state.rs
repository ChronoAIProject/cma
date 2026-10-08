//! Shared ingress clients and admission capacity.
use super::{launch::TurnLauncher, relay::jwks::JwksCache};
use crate::{channels::resolver::ChannelResolver, IngressConfig};
use cmaeg_core::seal::MasterKeyRing;
use std::sync::Arc;
use tokio::sync::Semaphore;
#[derive(Clone)]
pub struct IngressState {
    pub resolver: ChannelResolver,
    pub master_keys: Arc<MasterKeyRing>,
    pub jwks: Arc<JwksCache>,
    pub launcher: Arc<dyn TurnLauncher>,
    pub delivery: Arc<dyn crate::delivery::router::DeliverySink>,
    pub config: IngressConfig,
    pub routing: crate::RoutingConfig,
    pub observer: Arc<dyn crate::turns::observer::ActivityObserver>,
    pub interactions: Option<Arc<crate::interactions::InteractionStore>>,
    pub inflight: Arc<Semaphore>,
    pub telegram: Option<Arc<crate::telegram::TelegramClient>>,
    pub receipt_observer: Option<Arc<dyn super::receipts::PlatformReceiptObserver>>,
    /// Whether Telegram media is admitted; off, media is refused as before.
    pub media_enabled: bool,
}
impl IngressState {
    pub fn with_routing(
        mut self,
        config: crate::RoutingConfig,
        observer: Arc<dyn crate::turns::observer::ActivityObserver>,
    ) -> Self {
        self.routing = config;
        self.observer = observer;
        self
    }
    pub fn with_media_enabled(mut self, enabled: bool) -> Self {
        self.media_enabled = enabled;
        self
    }
    pub fn with_telegram(mut self, client: Arc<crate::telegram::TelegramClient>) -> Self {
        self.telegram = Some(client);
        self
    }
    pub fn with_receipt_observer(
        mut self,
        observer: Arc<dyn super::receipts::PlatformReceiptObserver>,
    ) -> Self {
        self.receipt_observer = Some(observer);
        self
    }
    pub fn with_interactions(mut self, store: Arc<crate::interactions::InteractionStore>) -> Self {
        self.interactions = Some(store);
        self
    }
    pub fn new(
        resolver: ChannelResolver,
        master_keys: Arc<MasterKeyRing>,
        config: IngressConfig,
        launcher: Arc<dyn TurnLauncher>,
        delivery: Arc<dyn crate::delivery::router::DeliverySink>,
    ) -> Result<Self, reqwest::Error> {
        Ok(Self {
            interactions: None,
            telegram: None,
            receipt_observer: None,
            media_enabled: false,
            routing: crate::RoutingConfig::default(),
            observer: Arc::new(crate::turns::observer::NoopObserver),
            resolver,
            master_keys,
            jwks: Arc::new(JwksCache::new(config.jwks_ttl)?),
            launcher,
            delivery,
            inflight: Arc::new(Semaphore::new(config.max_inflight)),
            config,
        })
    }
}
