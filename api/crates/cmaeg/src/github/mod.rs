mod config;
mod group;
pub mod http;
mod installations;
mod integrations;
mod model;
mod mutations;
mod receiver;
mod schema;
mod startup;
mod store;
mod worker;

pub use config::Config;
pub use integrations::{
    AppIntegration, AppList, CreateAppRequest, CredentialStatus, GitHubCapabilities, ListAppsQuery,
};
pub use model::{BindRequest, Binding, Receipt, Task, TaskResult, TaskResultStatus};
pub use mutations::{UpdateAppRequest, VerifyAppRequest, WebhookSecretRequest};
pub use schema::{migrate, migration_role, runtime_role};
pub(crate) use worker::Worker;

use std::sync::Arc;

use cmaeg_core::{activity::SourceKind, seal::MasterKeyRing};
use mongodb::Database;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub struct Gateway {
    config: Config,
    store: store::Store,
    ingress: Option<Arc<crate::ingress::IngressState>>,
    activity: Option<Arc<crate::activity::ActivityStore>>,
    providers: Option<Arc<crate::providers::ProviderClient>>,
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
    TooLarge,
    MediaType,
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Invalid => "invalid GitHub request",
            Self::Forbidden => "GitHub installation proof was refused",
            Self::NotFound => "GitHub resource was not found",
            Self::Signature => "GitHub webhook signature was refused",
            Self::Conflict => "GitHub record conflicts with an existing receipt",
            Self::ManagementConflict => {
                "GitHub integration request conflicts with a retained operation"
            }
            Self::VersionConflict(version) => {
                return write!(
                formatter,
                "GitHub integration version is {version}; read current metadata before updating"
            )
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
}

struct LifecycleDelivery<'a> {
    receipt_id: String,
    delivery_id: &'a str,
    fingerprint: &'a str,
    event: &'a str,
    repository_id: Option<u64>,
    payload: &'a serde_json::Value,
    body: &'a [u8],
}

struct DispatchInput<'a> {
    owner: &'a str,
    app_ref: &'a str,
    binding: &'a Binding,
    receipt_id: &'a str,
    delivery_id: &'a str,
    event: &'a str,
    action: Option<&'a str>,
    payload: &'a serde_json::Value,
}

impl Gateway {
    pub(crate) async fn from_records(
        records: &crate::records::RecordClient,
        keys: Arc<MasterKeyRing>,
    ) -> Option<Self> {
        let database = records.database()?;
        startup::initialize(database, keys).await
    }

    pub async fn open(
        database: Database,
        keys: Arc<MasterKeyRing>,
        config: Config,
    ) -> Result<Self, Error> {
        Ok(Self {
            config,
            store: store::Store::open(database, keys).await?,
            ingress: None,
            activity: None,
            providers: None,
        })
    }

    pub(crate) fn attach_ingress(&mut self, ingress: Arc<crate::ingress::IngressState>) {
        self.ingress = Some(ingress);
    }

    pub(crate) fn attach_activity(&mut self, activity: Arc<crate::activity::ActivityStore>) {
        self.activity = Some(activity);
    }

    pub(crate) fn attach_providers(&mut self, providers: Arc<crate::providers::ProviderClient>) {
        self.providers = Some(providers);
    }

    pub(crate) fn has_app(&self, app_ref: &str) -> bool {
        self.config.app(app_ref).is_some()
    }

    pub(crate) async fn task(&self, owner: &str, task_id: &str) -> Result<Task, Error> {
        let Some(ingress) = self.ingress.as_ref() else {
            return Err(Error::Unavailable);
        };
        let Some(routed) = ingress.routed.as_ref() else {
            return Err(Error::Unavailable);
        };
        match self
            .store
            .read::<group::ExecutionGroup>(task_id, "execution_group")
            .await
        {
            Ok((stored_owner, group)) => {
                if stored_owner != owner || group.owner != owner {
                    return Err(Error::NotFound);
                }
                return self.group_task(owner, group).await;
            }
            Err(Error::NotFound) => (),
            Err(error) => return Err(error),
        }
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

    async fn dispatch(&self, input: DispatchInput<'_>) -> Result<Option<Vec<String>>, Error> {
        let DispatchInput {
            owner,
            app_ref,
            binding,
            receipt_id,
            delivery_id,
            event,
            action,
            payload,
        } = input;
        let Some(routes) = self.config.routes(app_ref) else {
            return Ok(None);
        };
        if routes.is_empty() {
            return Ok(None);
        }
        let mut task_ids = Vec::new();
        for route in routes {
            if route.excludes(event, payload) {
                continue;
            }
            let Some(ingress) = self.ingress.as_ref() else {
                return Err(Error::Unavailable);
            };
            let channel = ingress
                .resolver
                .resolve(&route.channel_id)
                .await
                .map_err(|_| Error::Unavailable)?;
            if channel.definition.owner.subject != owner {
                return Err(Error::Forbidden);
            }
            let source_kind = SourceKind::Github {
                app_ref: app_ref.to_owned(),
                installation_id: binding.installation_id,
                repository_id: payload
                    .pointer("/repository/id")
                    .and_then(serde_json::Value::as_u64)
                    .ok_or(Error::Invalid)?,
            };
            let Some(source) = channel.definition.source_of(&source_kind) else {
                continue;
            };
            if !matches!(source, cmaeg_core::channel::Source::Github { .. })
                || !source.github_accepts(event, action)
            {
                continue;
            }
            let group_id = identity(
                "execution-group",
                &(
                    owner,
                    app_ref,
                    route.channel_id.as_str(),
                    channel.pointer.record_id.as_str(),
                ),
            );
            self.enqueue_group(
                &group_id,
                owner,
                app_ref,
                &channel.id.to_string(),
                channel.pointer.record_id.as_str(),
                group::Member {
                    receipt_id: receipt_id.into(),
                    delivery_id: delivery_id.into(),
                    event: event.into(),
                    action: action.map(str::to_owned),
                    installation_id: binding.installation_id,
                    authority_revision: binding.authority_revision,
                    repository_id: match source_kind {
                        SourceKind::Github { repository_id, .. } => repository_id,
                        _ => return Err(Error::Invalid),
                    },
                    payload: payload.clone(),
                },
            )
            .await?;
            task_ids.push(group_id);
        }
        Ok(Some(task_ids))
    }

    async fn enqueue_group(
        &self,
        group_id: &str,
        owner: &str,
        app_ref: &str,
        channel_id: &str,
        record_id: &str,
        member: group::Member,
    ) -> Result<(), Error> {
        for _ in 0..8 {
            match self
                .store
                .read_versioned::<group::ExecutionGroup>(group_id, "execution_group")
                .await
            {
                Ok((stored_owner, version, mut group)) => {
                    if stored_owner != owner
                        || group.owner != owner
                        || group.app_ref != app_ref
                        || group.channel_id != channel_id
                        || group.record_id != record_id
                    {
                        return Err(Error::Forbidden);
                    }
                    match group
                        .enqueue(member.clone(), now())
                        .map_err(map_group_error)?
                    {
                        group::EnqueueResult::Duplicate => return Ok(()),
                        group::EnqueueResult::Added { .. } => (),
                    }
                    match self
                        .store
                        .update(group_id, owner, "execution_group", version, &group)
                        .await
                    {
                        Ok(_) => return Ok(()),
                        Err(Error::Conflict) => continue,
                        Err(error) => return Err(error),
                    }
                }
                Err(Error::NotFound) => {
                    let mut group = group::ExecutionGroup::new(
                        group_id.into(),
                        owner.into(),
                        app_ref.into(),
                        channel_id.into(),
                        record_id.into(),
                        now(),
                    );
                    group
                        .enqueue(member.clone(), now())
                        .map_err(map_group_error)?;
                    match self
                        .store
                        .create(group_id, owner, "execution_group", &group)
                        .await
                    {
                        Ok(()) => return Ok(()),
                        Err(Error::Unavailable) => continue,
                        Err(error) => return Err(error),
                    }
                }
                Err(error) => return Err(error),
            }
        }
        Err(Error::Unavailable)
    }

    async fn binding(&self, owner: &str, id: &str) -> Result<Binding, Error> {
        let (stored_owner, binding): (String, Binding) = self.store.read(id, "binding").await?;
        if stored_owner != owner {
            return Err(Error::NotFound);
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

fn map_group_error(error: group::GroupError) -> Error {
    match error {
        group::GroupError::Invalid => Error::Invalid,
        group::GroupError::TooLarge | group::GroupError::Full => Error::TooLarge,
        group::GroupError::WrongBatch => Error::Conflict,
    }
}

fn identity(kind: &str, value: &impl Serialize) -> String {
    let bytes = serde_json::to_vec(&("github.com", kind, value)).expect("serializable identity");
    format!("{kind}_{:x}", Sha256::digest(bytes))
}
