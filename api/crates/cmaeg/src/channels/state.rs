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
use std::{collections::BTreeMap, fmt, sync::Arc};
use tokio::sync::Mutex;

/// Bounded management classifications; no caller-controlled metric labels.
#[derive(Clone, Copy)]
pub enum InvalidationPath {
    Rest,
    Recovery,
    Rekey,
    Delete,
    Console,
}
impl InvalidationPath {
    fn label(self) -> &'static str {
        match self {
            Self::Rest => "rest",
            Self::Recovery => "recovery",
            Self::Rekey => "rekey",
            Self::Delete => "delete",
            Self::Console => "console",
        }
    }
}

#[derive(Clone)]
pub struct ManagementState {
    pub resolver: super::resolver::ChannelResolver,
    pub readiness: crate::http::readiness::Readiness,
    pub providers: Arc<ProviderClient>,
    pub records: Arc<RecordClient>,
    pub creator_auth: Arc<CreatorAuthenticator>,
    pub master_keys: Arc<MasterKeyRing>,
    pub public_url: String,
    pub cma_base_url: String,
    pub tools_nyxid_slug: String,
    pub cma_service_token: cmaeg_core::channel::Secret,
    pub telegram: Option<Arc<crate::telegram::TelegramClient>>,
    /// Inbound media switch. Off, a custom source's attachments setting is
    /// refused, so it is stored only once every replica can parse it.
    pub media_enabled: bool,
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
    /// Count mutation invalidations even when the record has no cached aliases.
    pub fn invalidate_record(
        &self,
        record_id: &RecordId,
        also_id: Option<&str>,
        path: InvalidationPath,
    ) {
        self.resolver.invalidate_record(record_id, also_id);
        metrics::counter!(crate::telemetry::names::CHANNEL_INVALIDATIONS,
            crate::telemetry::names::PATH => path.label())
        .increment(1);
    }

    pub fn new(
        providers: Arc<ProviderClient>,
        records: Arc<RecordClient>,
        creator_auth: Arc<CreatorAuthenticator>,
        config: crate::ManagementConfig,
        cache_config: crate::ChannelCacheConfig,
    ) -> Self {
        let resolver = super::resolver::ChannelResolver::new(
            records.clone(),
            config.master_keys.clone(),
            cache_config,
        );
        Self {
            resolver,
            readiness: crate::http::readiness::Readiness::initialized(),
            providers,
            records,
            creator_auth,
            master_keys: config.master_keys,
            public_url: config.public_url,
            cma_base_url: config.cma_base_url,
            tools_nyxid_slug: config.tools_nyxid_slug,
            cma_service_token: config.cma_service_token,
            telegram: None,
            media_enabled: false,
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
    Conflict { expected: u64, actual: Option<u64> },
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
            Self::Conflict { expected, actual } => {
                write!(
                    formatter,
                    "channel version conflict: expected {expected}, actual {actual:?}"
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
