mod bindings;
pub mod http;
mod installations;
mod integrations;
mod model;
mod mutations;
mod receiver;
mod retry;
mod schema;
mod startup;
mod store;

pub use integrations::{
    AppIntegration, AppList, CreateAppRequest, CredentialStatus, GitHubCapabilities, ListAppsQuery,
};
pub use model::{
    BindRequest, Binding, BindingApproval, Receipt, Task, TaskResult, TaskResultStatus,
    VerifyBindingRequest,
};
pub use mutations::{RetireAppRequest, UpdateAppRequest, VerifyAppRequest, WebhookSecretRequest};
pub use schema::{migrate, migration_role, runtime_role};
pub(crate) use startup::Initialization;

use std::sync::Arc;

use cmaeg_core::seal::MasterKeyRing;
use mongodb::Database;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub struct Gateway {
    store: store::Store,
    ingress: Option<Arc<crate::ingress::IngressState>>,
    activity: Option<Arc<crate::activity::ActivityStore>>,
    recovery_cursor: Arc<tokio::sync::Mutex<Option<(String, String)>>>,
}

#[derive(Debug, Clone, Copy)]
pub enum Error {
    Invalid,
    Forbidden,
    NotFound,
    Signature,
    Conflict,
    ManagementConflict,
    VersionConflict(u64),
    Unavailable,
    Retry,
    DispatchUnavailable,
    Retired,
    ProofWithdrawn,
    DeliveryExpired,
    TooLarge,
    MediaType,
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Invalid => "invalid GitHub request",
            Self::Forbidden => "GitHub installation authority was refused",
            Self::NotFound => "GitHub resource was not found",
            Self::Signature => "GitHub webhook signature was refused",
            Self::Conflict => "GitHub record conflicts with an existing receipt",
            Self::ManagementConflict => {
                "GitHub integration request conflicts with a retained operation"
            }
            Self::VersionConflict(version) => {
                return write!(
                    formatter,
                    "GitHub resource version is {version}; read current metadata before updating"
                )
            }
            Self::ProofWithdrawn => {
                "Static installation proofs are withdrawn; request administrator approval"
            }
            Self::DeliveryExpired => {
                "GitHub delivery retention expired; use a fresh authorized reconciliation"
            }
            Self::Retired => "GitHub integration is retired",
            Self::DispatchUnavailable => "GitHub durable routed dispatch is unavailable",
            Self::Retry => {
                "GitHub authority changed during receipt commitment; retry the original delivery"
            }
            Self::Unavailable => "GitHub source storage is unavailable",
            Self::TooLarge => "GitHub webhook body exceeds 256 KiB",
            Self::MediaType => "GitHub webhook requires uncompressed UTF-8 application/json",
        })
    }
}

impl std::error::Error for Error {}

#[derive(Serialize, Deserialize)]
struct Delivery {
    receipt: Receipt,
    raw_body_records: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dispatch: Option<DispatchState>,
}

/// The only GitHub data allowed to cross the gateway/provider boundary.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GithubProjection {
    delivery_id: String,
    event: String,
    action: Option<String>,
    installation_id: u64,
    repository: GithubRepository,
    number: u64,
    html_url: String,
    sender: GithubSender,
    state: Option<String>,
    merged: Option<bool>,
    draft: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GithubRepository {
    id: u64,
    full_name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GithubSender {
    login: String,
    r#type: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DispatchState {
    schema_version: u8,
    projection: GithubProjection,
    channels: Vec<DispatchChannel>,
    #[serde(default)]
    lease: Option<DispatchLease>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DispatchChannel {
    channel_id: String,
    record_id: String,
    channel_version: u64,
    status: String,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    admission_id: Option<String>,
    #[serde(default)]
    attempts: u32,
    #[serde(default)]
    next_attempt_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DispatchLease {
    owner: String,
    until: i64,
}

struct LifecycleDelivery<'a> {
    integration: &'a AppIntegration,
    received_at: i64,
    receipt_id: String,
    delivery_id: &'a str,
    fingerprint: &'a str,
    event: &'a str,
    repository_id: Option<u64>,
    payload: &'a serde_json::Value,
    body: &'a [u8],
}

impl Gateway {
    pub(crate) async fn from_records(
        records: &crate::records::RecordClient,
        keys: Arc<MasterKeyRing>,
    ) -> Option<Self> {
        let database = records.database()?;
        startup::initialize(database, keys).await
    }

    pub async fn open(database: Database, keys: Arc<MasterKeyRing>) -> Result<Self, Error> {
        Ok(Self {
            store: store::Store::open(database, keys).await?,
            ingress: None,
            activity: None,
            recovery_cursor: Arc::new(tokio::sync::Mutex::new(None)),
        })
    }

    /// Supply an external retention clock; production uses system UTC.
    pub fn with_receipt_clock(mut self, clock: Arc<dyn Fn() -> i64 + Send + Sync>) -> Self {
        self.store.clock = clock;
        self
    }

    pub(crate) fn attach_ingress(&mut self, ingress: Arc<crate::ingress::IngressState>) {
        self.ingress = Some(ingress);
    }

    pub(crate) fn attach_activity(&mut self, activity: Arc<crate::activity::ActivityStore>) {
        self.activity = Some(activity);
    }

    pub(crate) async fn task(&self, owner: &str, task_id: &str) -> Result<Task, Error> {
        let Some(ingress) = self.ingress.as_ref() else {
            return Err(Error::Unavailable);
        };
        let Some(routed) = ingress.routed.as_ref() else {
            return Err(Error::Unavailable);
        };
        let mut task = routed.task(owner, task_id).await?;
        self.project_activity(owner, &mut task).await
    }

    async fn project_activity(&self, owner: &str, task: &mut Task) -> Result<Task, Error> {
        let Some(activity) = self.activity.as_ref() else {
            return Ok(task.clone());
        };
        let page = activity
            .latest(owner, &task.record_id)
            .await
            .map_err(|_| Error::Unavailable)?;
        if let Some(entry) = page
            .items
            .iter()
            .find(|entry| entry.event_id == task.event_id)
        {
            task.status = Task::status_for(&entry.consumers).into();
            task.response_id = entry
                .consumers
                .iter()
                .find_map(|consumer| consumer.response_id.clone());
        }
        Ok(task.clone())
    }

    pub(crate) async fn installation_binding(
        &self,
        owner: &str,
        app: &AppIntegration,
        installation_id: u64,
    ) -> Result<Binding, Error> {
        if app.status == "retired" {
            return Err(Error::Retired);
        }
        // New registrations never reuse a retired integration's binding identity.
        // Older bindings remain addressable under their original IDs and owner.
        let id = identity("binding", &(&app.app_ref, installation_id));
        let binding = match self.binding(owner, &id).await {
            Ok(binding) => binding,
            Err(Error::NotFound) => {
                self.binding(owner, &identity("binding", &(app.app_id, installation_id)))
                    .await?
            }
            Err(error) => return Err(error),
        };
        if binding.app_ref != app.app_ref
            || binding.app_id != app.app_id
            || binding.installation_id != installation_id
        {
            return Err(Error::NotFound);
        }
        Ok(binding)
    }

    async fn binding(&self, owner: &str, id: &str) -> Result<Binding, Error> {
        let (version, binding): (u64, Binding) =
            self.store.read_owned(owner, id, "binding").await?;
        if binding.binding_id != id
            || binding.version != version
            || binding.app_id == 0
            || binding.installation_id == 0
            || binding.account_id == 0
            || (identity("binding", &(&binding.app_ref, binding.installation_id)) != id
                && identity("binding", &(binding.app_id, binding.installation_id)) != id)
        {
            return Err(Error::Unavailable);
        }
        Ok(binding)
    }
}

fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

fn duplicate(delivery: Delivery, event: &str, fingerprint: &str) -> Result<(Receipt, bool), Error> {
    if delivery.receipt.event != event || delivery.receipt.body_sha256 != fingerprint {
        return Err(Error::Conflict);
    }
    Ok((delivery.receipt, false))
}

fn identity(kind: &str, value: &impl Serialize) -> String {
    let bytes = serde_json::to_vec(&("github.com", kind, value)).expect("serializable identity");
    format!("{kind}_{:x}", Sha256::digest(bytes))
}

fn valid_ref(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}
