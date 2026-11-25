//! Shared ingress clients and admission capacity.
use super::{launch::TurnLauncher, relay::jwks::JwksCache};
use crate::{
    channels::resolver::ChannelResolver,
    turns::observer::{ActivityObserver, NoopObserver},
    IngressConfig,
};
use cmaeg_core::seal::MasterKeyRing;
use std::sync::Arc;
use tokio::sync::Semaphore;
#[derive(Clone)]
pub struct IngressState {
    pub native_lark_intake_enabled: bool,
    pub native_lark_media_enabled: bool,
    pub native_lark_stickers_enabled: bool,
    pub resolver: ChannelResolver,
    pub profile_revisions_enabled: bool,
    pub master_keys: Arc<MasterKeyRing>,
    pub jwks: Arc<JwksCache>,
    pub launcher: Arc<dyn TurnLauncher>,
    pub routed: Option<Arc<super::routed::RoutedIngress>>,
    pub config: IngressConfig,
    pub interactions: Option<Arc<crate::interactions::InteractionStore>>,
    pub inflight: Arc<Semaphore>,
    pub telegram: Option<Arc<crate::telegram::TelegramClient>>,
    pub whatsapp_receipts: Option<Arc<crate::whatsapp::receipts::Store>>,
    pub receipt_observer: Option<Arc<dyn super::receipts::PlatformReceiptObserver>>,
    /// Shared media admission; native WhatsApp additionally requires its issuance gate.
    pub media_enabled: bool,
    /// Reader-first native WhatsApp locator issuance; retained reads remain available.
    pub native_whatsapp_media_enabled: bool,
    pub native_whatsapp_groups_enabled: bool,
    pub native_whatsapp_message_events_enabled: bool,
    pub telegram_effects_issuance_enabled: bool,
    pub native_telegram_member_joins_enabled: bool,
    pub relay_telegram_member_joins_enabled: bool,
    /// Receives authenticated refusals, producers and authentication
    /// failures; a no-op unless the console is on.
    pub activity: Arc<dyn ActivityObserver>,
}
impl IngressState {
    pub fn with_whatsapp_message_events(mut self, enabled: bool) -> Self {
        self.native_whatsapp_message_events_enabled = enabled;
        self
    }
    pub fn with_native_lark_stickers(mut self, enabled: bool) -> Self {
        self.native_lark_stickers_enabled = enabled;
        self
    }
    pub fn with_native_lark_media(mut self, enabled: bool) -> Self {
        self.native_lark_media_enabled = enabled;
        self
    }
    pub fn with_native_lark_intake(mut self, enabled: bool) -> Self {
        self.native_lark_intake_enabled = enabled;
        self
    }
    pub fn with_whatsapp_groups(mut self, enabled: bool) -> Self {
        self.native_whatsapp_groups_enabled = enabled;
        self
    }
    pub fn with_whatsapp_media(mut self, enabled: bool) -> Self {
        self.native_whatsapp_media_enabled = enabled;
        self
    }
    pub(crate) fn with_profile_revisions(mut self, enabled: bool) -> Self {
        self.profile_revisions_enabled = enabled;
        self
    }
    pub fn with_routed(mut self, routed: Option<Arc<super::routed::RoutedIngress>>) -> Self {
        self.routed = routed;
        self
    }
    pub fn with_whatsapp_receipts(mut self, store: Arc<crate::whatsapp::receipts::Store>) -> Self {
        self.whatsapp_receipts = Some(store);
        self
    }
    pub fn with_telegram_effects(mut self, enabled: bool) -> Self {
        self.telegram_effects_issuance_enabled = enabled;
        self
    }
    pub fn with_activity_observer(mut self, observer: Arc<dyn ActivityObserver>) -> Self {
        self.activity = observer;
        self
    }

    pub(crate) async fn launch_github(
        &self,
        request: super::launch::LaunchRequest,
        body: Vec<u8>,
        authority: super::routed::github::GithubAuthorityFence,
    ) -> super::launch::LaunchOutcome {
        let Some(routed) = self.routed.as_ref() else {
            return super::launch::LaunchOutcome::Refused(cmaeg_core::problem::Problem::new(
                cmaeg_core::problem::ProblemCode::ProviderUnavailable,
                503,
                "github_routing_unavailable",
            ));
        };
        let Ok(hold) = self.inflight.clone().acquire_owned().await else {
            return super::launch::LaunchOutcome::Refused(cmaeg_core::problem::Problem::new(
                cmaeg_core::problem::ProblemCode::ProviderUnavailable,
                503,
                "github_ingress_unavailable",
            ));
        };
        routed
            .launch_github_verified(
                request,
                body,
                hold,
                self.activity.as_ref(),
                &self.resolver,
                authority,
            )
            .await
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
    ) -> Result<Self, reqwest::Error> {
        Ok(Self {
            native_lark_intake_enabled: false,
            native_lark_media_enabled: false,
            native_lark_stickers_enabled: false,
            routed: None,
            profile_revisions_enabled: false,
            interactions: None,
            telegram: None,
            whatsapp_receipts: None,
            receipt_observer: None,
            media_enabled: false,
            native_whatsapp_media_enabled: false,
            native_whatsapp_groups_enabled: false,
            native_whatsapp_message_events_enabled: false,
            telegram_effects_issuance_enabled: false,
            native_telegram_member_joins_enabled: false,
            relay_telegram_member_joins_enabled: false,
            activity: Arc::new(NoopObserver),
            resolver,
            master_keys,
            jwks: Arc::new(JwksCache::new(config.jwks_ttl)?),
            launcher,
            inflight: Arc::new(Semaphore::new(config.max_inflight)),
            config,
        })
    }
}
