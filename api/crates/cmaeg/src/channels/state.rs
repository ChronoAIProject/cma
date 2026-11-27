//! Shared management clients, process-local state, and result types.
use crate::{
    auth::{CreatorAuthState, CreatorAuthenticator},
    providers::{BindingResponse, ProviderClient, ProviderError},
    records::{RecordClient, RecordError, RecordValue},
};
use cmaeg_core::{
    channel::ChannelDefinition,
    ids::{ChannelId, RecordId},
    seal::{ChannelPointer, MasterKeyRing},
    ValidationError,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::{Arc, OnceLock},
};
use tokio::sync::Mutex;
#[derive(Clone)]
pub struct ManagementState {
    pub managed_telegram_setup_enabled: bool,
    pub github_enabled: bool,
    pub github: Arc<OnceLock<Arc<crate::github::Gateway>>>,
    pub native_lark_controls_enabled: bool,
    pub native_lark_forms_enabled: bool,
    pub native_lark_intake_enabled: bool,
    pub native_lark_writes_enabled: bool,
    pub native_lark_admission_enabled: bool,
    pub native_lark: Option<Arc<crate::native_lark::Client>>,
    pub activity: Arc<crate::activity::ActivityStore>,
    pub resolver: super::resolver::ChannelResolver,
    pub readiness: crate::http::readiness::Readiness,
    pub providers: Arc<ProviderClient>,
    pub records: Arc<RecordClient>,
    pub creator_auth: Arc<CreatorAuthenticator>,
    pub master_keys: Arc<MasterKeyRing>,
    pub public_url: String,
    pub console_public_url: String,
    pub public_api_url: String,
    pub public_api_aliases: Vec<String>,
    pub advertise_public_api_url: bool,
    pub cma_base_url: String,
    pub tools_nyxid_slug: String,
    pub cma_service_token: cmaeg_core::channel::Secret,
    pub telegram: Option<Arc<crate::telegram::TelegramClient>>,
    /// Allows new native bot mention opt-ins once every reader and consumer
    /// supports the policy. Does not disable execution of retained opt-ins.
    pub native_bot_mentions_writes_enabled: bool,
    /// Allows new thread contracts only after every reader supports them.
    pub thread_contract_writes_enabled: bool,
    /// Inbound media switch. Off, a custom source's attachments setting is
    /// refused, so it is stored only once every replica can parse it.
    pub media_enabled: bool,
    /// Writer gate only; retained opt-ins continue through the durable reader.
    pub custom_durable_execution_enabled: bool,
    /// Providers whose existing-Agent original-media continuation path is ready.
    pub custom_existing_agent_media_providers: BTreeSet<String>,
    pub multi_consumer_writes_enabled: bool,
    pub profile_revisions_enabled: bool,
    pub dynamic_sources_writes_enabled: bool,
    pub relay_raw_verification_writes_enabled: bool,
    pub lark_resource_writes_enabled: bool,
    pub native_whatsapp_groups_writes_enabled: bool,
    pub native_whatsapp_message_events_enabled: bool,
    pub native_whatsapp_forms_enabled: bool,
    pub native_whatsapp_flow_writes_enabled: bool,
    pub native_whatsapp_writes_enabled: bool,
    pub native_whatsapp_admission_enabled: bool,
    pub whatsapp: Option<Arc<crate::whatsapp::Client>>,
    pub lark_message_effects_writes_enabled: bool,
    pub native_discord_writes_enabled: bool,
    pub native_telegram_message_events_writes_enabled: bool,
    pub native_telegram_member_joins_enabled: bool,
    pub relay_telegram_member_joins_enabled: bool,
    pub native_telegram_durable_admission_enabled: bool,
    pub native_discord_thread_history_enabled: bool,
    pub native_telegram_topic_lifecycle_enabled: bool,
    pub native_telegram_topic_creation_enabled: bool,
    pub native_telegram_general_topic_lifecycle_enabled: bool,
    pub native_discord_thread_lifecycle_enabled: bool,
    pub native_lark_conversation_history_enabled: bool,
    pub native_discord_conversation_history_enabled: bool,
    pub native_discord_reactions_enabled: bool,
    pub native_discord_rich_content_enabled: bool,
    pub native_discord_forwards_enabled: bool,
    pub native_discord: crate::discord::selection::Selection,
    pub native_discord_admission_enabled: bool,
    pub outbound_messages_writes_enabled: bool,
    pub outbound_relay: Option<crate::delivery::relay_client::RelayClient>,
    /// Receives every record head management writes or deletes; a no-op
    /// unless the console is on.
    pub index: Arc<dyn super::observer::ChannelIndexObserver>,
    mutations: Arc<[Mutex<()>; 64]>,
    pub(crate) idempotency: Arc<Mutex<BTreeMap<(String, String), CachedResponse>>>,
}

#[derive(Debug, Clone)]
pub(crate) struct CachedResponse {
    pub expires_at: std::time::Instant,
    pub body: serde_json::Value,
    pub fingerprint: String,
}

impl ManagementState {
    pub(crate) fn telegram_member_join_authoring(&self) -> bool {
        self.native_telegram_member_joins_enabled
            && self.native_telegram_durable_admission_enabled
            && self.multi_consumer_writes_enabled
    }
    pub(crate) fn relay_telegram_member_join_authoring(&self) -> bool {
        self.relay_telegram_member_joins_enabled
            && self.native_telegram_durable_admission_enabled
            && self.multi_consumer_writes_enabled
    }
    pub(crate) fn write_flags(&self) -> super::write_gate::WriteFlags {
        super::write_gate::WriteFlags {
            github: self.github_enabled,
            durable_custom_execution: self.custom_durable_execution_enabled,
            native_lark: self.native_lark_writes_enabled,
            native_whatsapp_message_events: self.native_whatsapp_message_events_enabled
                && self.native_whatsapp_admission_enabled,
            native_whatsapp_forms: self.native_whatsapp_forms_enabled
                && self.native_whatsapp_flow_writes_enabled
                && self.native_whatsapp_admission_enabled,
            native_bot_mentions: self.native_bot_mentions_writes_enabled,
            thread_contract: self.thread_contract_writes_enabled,
            multi_consumer: self.multi_consumer_writes_enabled,
            dynamic_sources: self.dynamic_sources_writes_enabled,
            relay_raw_verification: self.relay_raw_verification_writes_enabled,
            lark_resources: self.lark_resource_writes_enabled,
            native_whatsapp_groups: self.native_whatsapp_groups_writes_enabled,
            native_whatsapp: self.native_whatsapp_writes_enabled,
            lark_message_effects: self.lark_message_effects_writes_enabled,
            native_discord: self.native_discord_writes_enabled,
            telegram_message_events: self.native_telegram_message_events_writes_enabled,
            telegram_member_joins: self.telegram_member_join_authoring(),
            relay_telegram_member_joins: self.relay_telegram_member_join_authoring(),
            discord_thread_history: self.native_discord_thread_history_enabled,
            telegram_topic_lifecycle: self.native_telegram_topic_lifecycle_enabled,
            telegram_topic_creation: self.native_telegram_topic_creation_enabled,
            discord_thread_lifecycle: self.native_discord_thread_lifecycle_enabled,
            native_lark_conversation_history: self.native_lark_conversation_history_enabled,
            discord_conversation_history: self.native_discord_conversation_history_enabled,
            discord_reactions: self.native_discord_reactions_enabled,
        }
    }

    pub(crate) fn public_api_bases(&self) -> impl Iterator<Item = &str> {
        [self.public_api_url.as_str(), self.public_url.as_str()]
            .into_iter()
            .chain(self.public_api_aliases.iter().map(String::as_str))
    }

    pub fn advertised_api_url(&self) -> &str {
        if self.advertise_public_api_url {
            &self.public_api_url
        } else {
            &self.public_url
        }
    }

    pub fn new(
        providers: Arc<ProviderClient>,
        records: Arc<RecordClient>,
        creator_auth: Arc<CreatorAuthenticator>,
        config: crate::ManagementConfig,
        cache_config: crate::ChannelCacheConfig,
    ) -> Self {
        Self::observed(
            providers,
            records,
            creator_auth,
            config,
            cache_config,
            Arc::new(crate::turns::observer::NoopObserver),
        )
    }

    /// Management state whose writes, deletes and restores report to `index`.
    pub fn observed(
        providers: Arc<ProviderClient>,
        records: Arc<RecordClient>,
        creator_auth: Arc<CreatorAuthenticator>,
        config: crate::ManagementConfig,
        cache_config: crate::ChannelCacheConfig,
        index: Arc<dyn super::observer::ChannelIndexObserver>,
    ) -> Self {
        let resolver = super::resolver::ChannelResolver::observed(
            records.clone(),
            config.master_keys.clone(),
            cache_config,
            index.clone(),
        );
        Self {
            github: Arc::new(OnceLock::new()),
            github_enabled: false,
            activity: Arc::new(crate::activity::ActivityStore::new(
                (*records).clone(),
                config.master_keys.clone(),
            )),
            resolver,
            readiness: crate::http::readiness::Readiness::initialized(),
            providers,
            records,
            creator_auth,
            console_public_url: config.console_origin().to_owned(),
            master_keys: config.master_keys,
            public_url: config.public_url,
            public_api_url: config.public_api_url,
            public_api_aliases: config.public_api_aliases,
            advertise_public_api_url: config.advertise_public_api_url,
            cma_base_url: config.cma_base_url,
            tools_nyxid_slug: config.tools_nyxid_slug,
            cma_service_token: config.cma_service_token,
            telegram: None,
            managed_telegram_setup_enabled: false,
            native_bot_mentions_writes_enabled: false,
            thread_contract_writes_enabled: false,
            media_enabled: false,
            custom_durable_execution_enabled: false,
            custom_existing_agent_media_providers: BTreeSet::new(),
            multi_consumer_writes_enabled: false,
            profile_revisions_enabled: false,
            dynamic_sources_writes_enabled: false,
            relay_raw_verification_writes_enabled: false,
            lark_resource_writes_enabled: false,
            native_lark_controls_enabled: false,
            native_lark_forms_enabled: false,
            native_lark_intake_enabled: false,
            native_lark_writes_enabled: false,
            native_lark_admission_enabled: false,
            native_lark: None,
            native_whatsapp_message_events_enabled: false,
            native_whatsapp_forms_enabled: false,
            native_whatsapp_flow_writes_enabled: false,
            native_whatsapp_writes_enabled: false,
            native_whatsapp_groups_writes_enabled: false,
            native_whatsapp_admission_enabled: false,
            whatsapp: None,
            lark_message_effects_writes_enabled: false,
            native_discord_writes_enabled: false,
            native_telegram_message_events_writes_enabled: false,
            native_telegram_member_joins_enabled: false,
            relay_telegram_member_joins_enabled: false,
            native_telegram_durable_admission_enabled: false,
            native_discord_thread_history_enabled: false,
            native_telegram_topic_lifecycle_enabled: false,
            native_telegram_topic_creation_enabled: false,
            native_telegram_general_topic_lifecycle_enabled: false,
            native_discord_thread_lifecycle_enabled: false,
            native_lark_conversation_history_enabled: false,
            native_discord_conversation_history_enabled: false,
            native_discord_reactions_enabled: false,
            native_discord_rich_content_enabled: false,
            native_discord_forwards_enabled: false,
            native_discord: Default::default(),
            native_discord_admission_enabled: false,
            outbound_messages_writes_enabled: false,
            outbound_relay: None,
            index,
            idempotency: Arc::new(Mutex::new(BTreeMap::new())),
            mutations: Arc::new(std::array::from_fn(|_| Mutex::new(()))),
        }
    }

    pub(crate) async fn mutation_guard(&self, key: &str) -> tokio::sync::MutexGuard<'_, ()> {
        use std::hash::{Hash, Hasher};
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        key.hash(&mut hash);
        self.mutations[hash.finish() as usize % self.mutations.len()]
            .lock()
            .await
    }
}

impl CreatorAuthState for ManagementState {
    fn creator_auth(&self) -> &CreatorAuthenticator {
        &self.creator_auth
    }
}

#[derive(Debug)]
pub enum ManagementError {
    Invalid(ValidationError),
    Provider(ProviderError),
    Record(RecordError),
    NotFound,
    CreationPending,
    Conflict {
        expected: u64,
        actual: Option<u64>,
    },
    /// The provider answered a binding release with a definite refusal. The
    /// channel record is retained; nothing proves the binding is absent.
    ReleaseRefused {
        status: u16,
        code: String,
    },
    Unauthorized,
    Internal(String),
}

impl fmt::Display for ManagementError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(error) => write!(formatter, "{error}"),
            Self::Provider(error) => write!(formatter, "{error}"),
            Self::Record(error) => write!(formatter, "{error}"),
            Self::NotFound => formatter.write_str("channel was not found"),
            Self::CreationPending => formatter.write_str(super::creation::PENDING_DETAIL),
            Self::Conflict { expected, actual } => {
                write!(
                    formatter,
                    "channel version conflict: expected {expected}, actual {actual:?}"
                )
            }
            Self::ReleaseRefused { status, .. } => {
                write!(
                    formatter,
                    "provider refused to release the binding (HTTP {status})"
                )
            }
            Self::Unauthorized => formatter.write_str("management service authentication failed"),
            Self::Internal(detail) => formatter.write_str(detail),
        }
    }
}

impl std::error::Error for ManagementError {}

#[derive(Debug)]
pub struct CreatedChannel {
    pub channel_id: ChannelId,
    pub record_id: RecordId,
    pub definition: ChannelDefinition,
    pub version: u64,
    pub binding: BindingResponse,
}

#[derive(Debug)]
pub struct LoadedChannel {
    pub channel_id: ChannelId,
    pub pointer: ChannelPointer,
    pub definition: ChannelDefinition,
    pub record: RecordValue,
}

impl LoadedChannel {
    pub(crate) fn create_replayable(&self) -> bool {
        self.definition
            .creation
            .as_ref()
            .is_some_and(|creation| creation.ready)
            && !self.definition.mutation.as_ref().is_some_and(|mutation| {
                mutation.operation == cmaeg_core::channel::ChannelMutationKind::Delete
            })
    }
}

/// A channel read by its record id; see
/// [`super::management::load_by_record`].
#[derive(Debug)]
pub struct RecordedChannel {
    pub definition: ChannelDefinition,
    pub version: u64,
    /// The sealed record's size, against CMA's 64 KiB record limit.
    pub record_bytes: usize,
    /// The admitted version of an owner-retired incomplete channel.
    pub retirement_expected_version: Option<u64>,
    /// Whether the definition passes this image's reader rules.
    pub valid: bool,
}
