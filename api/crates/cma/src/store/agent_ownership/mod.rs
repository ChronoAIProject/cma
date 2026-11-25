//! Majority-backed membership and permanent, monotonically fenced owner rows.

use futures_util::TryStreamExt;
use layer2_core::agent_ownership::{
    Member, Membership, OwnershipError, OwnershipFailure, OwnershipLease, OwnershipStore,
    OwnershipToken, ReconciliationOwner, DEFAULT_LEASE_MS, PROTOCOL_VERSION,
};
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::AgentId;
use mongodb::bson::{doc, Document};
use mongodb::{Client, ClientSession, Collection, Database};

use super::durability::majority_collection;
use super::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, start_transaction, CommitOutcome,
};

mod acquisition;
mod binding_diagnostics;
mod effects;
mod failure;
mod membership;
mod records;
mod renewal_batch;
mod work_discovery;
use records::{member_from, owner_from, token_filter};

pub(super) const MEMBERS: &str = "cma_agent_manager_members";
pub(super) const OWNERS: &str = "cma_agent_manager_owners";

#[derive(Clone)]
pub(crate) struct MongoAgentOwnership {
    client: Client,
    database: Database,
    members: Collection<Document>,
    owners: Collection<Document>,
    receiver_diagnostics_enabled: bool,
}

/// Preserve driver labels until the owning transaction decides whether it can retry.
pub(in crate::store) enum FenceError {
    Domain(SandboxStoreError),
    Driver(mongodb::error::Error),
}

pub(in crate::store) enum ObservedFenceError {
    Read(mongodb::error::Error),
    Fence(FenceError),
}

impl ObservedFenceError {
    pub(in crate::store) fn into_fence_error(self) -> FenceError {
        match self {
            Self::Read(error) => FenceError::Driver(error),
            Self::Fence(error) => error,
        }
    }
}

impl FenceError {
    /// Callers without their own bounded retry keep the historical classification.
    pub(in crate::store) fn into_store(self) -> SandboxStoreError {
        match self {
            Self::Domain(error) => error,
            Self::Driver(error) => fence_write_error(error),
        }
    }
}

impl MongoAgentOwnership {
    pub(crate) async fn open(client: &Client, name: &str) -> Result<Self, OwnershipError> {
        let receiver_diagnostics_enabled = failure::diagnostics_enabled(std::env::var(
            "CHRONO_AGENT_MANAGER_OWNERSHIP_DIAGNOSTICS_ENABLED",
        ))?;
        let database = client.database(name);
        super::schema::initialize_collections(&database, &[MEMBERS, OWNERS])
            .await
            .map_err(unavailable)?;
        let store = Self::from_database(client, database, receiver_diagnostics_enabled);
        store
            .members
            .create_index(
                mongodb::IndexModel::builder()
                    .keys(doc! {"expires_at": 1})
                    .options(
                        mongodb::options::IndexOptions::builder()
                            .name("agent_manager_members_expiry".to_owned())
                            .expire_after(std::time::Duration::ZERO)
                            .build(),
                    )
                    .build(),
            )
            .await
            .map_err(unavailable)?;
        // Never expire owner rows: their epoch is the permanent high watermark.
        store
            .owners
            .create_index(
                mongodb::IndexModel::builder()
                    .keys(doc! {"instance_id": 1, "expires_at": 1})
                    .build(),
            )
            .await
            .map_err(unavailable)?;
        Ok(store)
    }

    fn from_database(
        client: &Client,
        database: Database,
        receiver_diagnostics_enabled: bool,
    ) -> Self {
        Self {
            client: client.clone(),
            receiver_diagnostics_enabled,
            members: majority_collection(&database, MEMBERS),
            owners: majority_collection(&database, OWNERS),
            database,
        }
    }

    #[cfg(feature = "qualification-hooks")]
    pub(crate) async fn prepare_qualification_renewal(
        uri: &str,
        name: &str,
    ) -> Result<std::sync::Arc<dyn OwnershipStore>, &'static str> {
        let mut options = mongodb::options::ClientOptions::parse(uri)
            .await
            .map_err(|_| "qualification MongoDB configuration refused")?;
        options.app_name = Some("cma-qualification-renewal".into());
        options.min_pool_size = Some(0);
        options.max_pool_size = Some(1);
        super::transaction_barrier::bind(&mut options, name);
        let client =
            Client::with_options(options).map_err(|_| "qualification MongoDB client refused")?;
        let database = client.database(name);
        // Open and verify this separate pool before the local plan can arm.
        // The ordinary composition owns collections, indexes and all other calls.
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            database.run_command(doc! {"ping": 1}),
        )
        .await
        .map_err(|_| "qualification MongoDB startup timed out")?
        .map_err(|_| "qualification MongoDB startup failed")?;
        Ok(std::sync::Arc::new(Self::from_database(
            &client, database, false,
        )))
    }

    async fn server_now(&self) -> Result<i64, OwnershipError> {
        self.database
            .run_command(doc! {"hello": 1})
            .await
            .map_err(unavailable)?
            .get_datetime("localTime")
            .map(|time| time.timestamp_millis())
            .map_err(invalid)
    }

    /// The driver aborts a dropped session only after its transaction's first
    /// command has returned. A caller cancelled while a first-command write is
    /// in flight would leave that write's locks on the server until the session
    /// is reused or the transaction lifetime expires (#3189). On the member row
    /// that would block every effect fence of this instance. Reading first
    /// strands at most a lock-free snapshot; the driver aborts any later drop.
    pub(super) async fn open_with_read(
        &self,
        instance: &str,
        session: &mut ClientSession,
    ) -> mongodb::error::Result<()> {
        self.members
            .find_one(doc! {"_id": instance})
            .projection(doc! {"_id": 1})
            .session(session)
            .await
            .map(|_| ())
    }

    /// A write in the effect's own transaction creates a conflict with takeover.
    /// A preceding read, even a majority read, would leave a TOCTOU gap.
    #[cfg(test)]
    pub(crate) async fn fence_transaction(
        &self,
        token: &OwnershipToken,
        session: &mut ClientSession,
    ) -> Result<(), SandboxStoreError> {
        self.fence_transaction_typed(token, session)
            .await
            .map_err(FenceError::into_store)
    }

    pub(in crate::store) async fn fence_transaction_typed(
        &self,
        token: &OwnershipToken,
        session: &mut ClientSession,
    ) -> Result<(), FenceError> {
        self.fence_transaction_observed(token, session)
            .await
            .map_err(ObservedFenceError::into_fence_error)
    }

    pub(super) async fn fence_transaction_observed(
        &self,
        token: &OwnershipToken,
        session: &mut ClientSession,
    ) -> Result<(), ObservedFenceError> {
        self.open_with_read(&token.instance_id, session)
            .await
            .map_err(ObservedFenceError::Read)?;
        self.fence_transaction_writes(token, session)
            .await
            .map_err(ObservedFenceError::Fence)
    }

    async fn fence_transaction_writes(
        &self,
        token: &OwnershipToken,
        session: &mut ClientSession,
    ) -> Result<(), FenceError> {
        let member = self
            .members
            .update_one(
                doc! {
                    "_id": &token.instance_id, "draining": false,
                    "protocol": i64::from(PROTOCOL_VERSION),
                    "$expr": {"$gt": ["$expires_at", "$$NOW"]},
                },
                doc! {"$inc": {"effect_revision": 1_i64}},
            )
            .session(&mut *session)
            .await
            .map_err(FenceError::Driver)?;
        if member.matched_count != 1 {
            return Err(FenceError::Domain(SandboxStoreError::Conflict));
        }
        let matched = self
            .owners
            .update_one(
                token_filter(token).map_err(|_| FenceError::Domain(SandboxStoreError::Conflict))?,
                doc! {"$inc": {"effect_revision": 1_i64}},
            )
            .session(&mut *session)
            .await
            .map_err(FenceError::Driver)?
            .matched_count;
        if matched == 1 {
            Ok(())
        } else {
            Err(FenceError::Domain(SandboxStoreError::Conflict))
        }
    }
}

#[async_trait::async_trait]
impl OwnershipStore for MongoAgentOwnership {
    fn receiver_diagnostics_enabled(&self) -> bool {
        self.receiver_diagnostics_enabled
    }

    async fn register(&self, member: Member) -> Result<Membership, OwnershipError> {
        self.register_member(member).await
    }

    async fn heartbeat(
        &self,
        instance_id: &str,
        draining: bool,
    ) -> Result<Membership, OwnershipError> {
        self.heartbeat_member(instance_id, draining).await
    }

    async fn membership(&self) -> Result<Membership, OwnershipError> {
        let now_ms = self.server_now().await?;
        let rows: Vec<Document> = self
            .members
            .find(doc! {"$expr": {"$gt": ["$expires_at", "$$NOW"]}})
            .await
            .map_err(unavailable)?
            .try_collect()
            .await
            .map_err(unavailable)?;
        let membership = Membership {
            now_ms,
            members: rows.iter().map(member_from).collect::<Result<_, _>>()?,
        };
        membership.require_compatible()?;
        Ok(membership)
    }

    async fn owner(&self, id: &AgentId) -> Result<Option<OwnershipLease>, OwnershipError> {
        self.owners
            .find_one(doc! {
                "_id": id.as_str(), "instance_id": {"$type": "string"},
                "$expr": {"$gt": ["$expires_at", "$$NOW"]},
            })
            .await
            .map_err(unavailable)?
            .as_ref()
            .map(owner_from)
            .transpose()
    }

    async fn reconciliation_owners(
        &self,
        ids: &[&AgentId],
    ) -> Result<Vec<ReconciliationOwner>, OwnershipError> {
        let ids: Vec<_> = ids.iter().map(|id| id.as_str()).collect();
        let mut rows = self
            .owners
            .aggregate([
                doc! {"$match": {
                    "_id": {"$in": &ids}, "instance_id": {"$type": "string"},
                    "expires_at": {"$type": "date"},
                    "$expr": {"$gt": ["$expires_at", "$$NOW"]},
                }},
                doc! {"$project": {
                    "document_schema_version": 1, "instance_id": 1, "epoch": 1, "protocol": 1,
                    "expires_at": 1, "remaining_ms": {"$subtract": ["$expires_at", "$$NOW"]},
                }},
            ])
            .batch_size(ids.len() as u32 + 1)
            .await
            .map_err(unavailable)?;
        let mut owners = Vec::new();
        while let Some(row) = rows.try_next().await.map_err(unavailable)? {
            let (Ok(lease), Some(remaining_ms)) = (
                owner_from(&row),
                row.get_i64("remaining_ms")
                    .ok()
                    .and_then(|millis| u64::try_from(millis).ok()),
            ) else {
                continue;
            };
            owners.push(ReconciliationOwner {
                token: lease.token,
                remaining_ms,
            });
        }
        Ok(owners)
    }

    async fn has_owner_history(&self, id: &AgentId) -> Result<bool, OwnershipError> {
        self.owners
            .find_one(doc! {"_id": id.as_str()})
            .projection(doc! {"_id": 1})
            .await
            .map(|row| row.is_some())
            .map_err(unavailable)
    }

    async fn acquire(
        &self,
        agent_id: &AgentId,
        instance_id: &str,
    ) -> Result<Option<OwnershipLease>, OwnershipError> {
        self.acquire_owner(agent_id, instance_id).await
    }

    async fn reconcile_acquisition(
        &self,
        agent_id: &AgentId,
        instance_id: &str,
    ) -> Result<Option<OwnershipToken>, OwnershipError> {
        self.reconcile_owner_acquisition(agent_id, instance_id)
            .await
    }

    async fn renew(&self, token: &OwnershipToken) -> Result<OwnershipLease, OwnershipError> {
        self.renew_owner(token).await
    }

    fn supports_batch_renewal(&self) -> bool {
        true
    }

    async fn renew_batch(
        &self,
        tokens: &[OwnershipToken],
    ) -> Result<layer2_core::agent_ownership::RenewalBatch, OwnershipError> {
        self.renew_owner_batch(tokens).await
    }

    async fn release(&self, token: &OwnershipToken) -> Result<(), OwnershipError> {
        self.release_owner(token).await
    }

    async fn check(&self, token: &OwnershipToken) -> Result<(), OwnershipError> {
        self.members
            .find_one(doc! {
                "_id": &token.instance_id, "draining": false,
                "protocol": i64::from(PROTOCOL_VERSION),
                "$expr": {"$gt": ["$expires_at", "$$NOW"]},
            })
            .await
            .map_err(unavailable)?
            .ok_or(OwnershipError::Stale)?;
        self.owners
            .find_one(token_filter(token)?)
            .await
            .map_err(unavailable)?
            .ok_or(OwnershipError::Stale)?;
        Ok(())
    }

    async fn record_receiver_failure(
        &self,
        token: &OwnershipToken,
        failure: OwnershipFailure,
    ) -> Result<(), OwnershipError> {
        self.record_failure(token, failure).await
    }
}

fn unavailable(error: impl std::fmt::Display) -> OwnershipError {
    OwnershipError::Unavailable(error.to_string())
}

fn invalid(error: impl std::fmt::Display) -> OwnershipError {
    OwnershipError::Invalid(error.to_string())
}

#[cfg(test)]
mod mongo_failure_containment_tests;
#[cfg(test)]
pub(crate) mod mongo_renewal_faults;
#[cfg(test)]
mod mongo_tests;

// This only classifies writes inside an uncommitted effect transaction. A
// transient transaction error proves abort; commit uncertainty never does.
fn fence_write_error(error: mongodb::error::Error) -> SandboxStoreError {
    if known_aborted(&error) {
        SandboxStoreError::Conflict
    } else {
        SandboxStoreError::Unavailable(error.to_string())
    }
}

// An unknown commit label always wins over a transient-abort label.
fn known_aborted(error: &mongodb::error::Error) -> bool {
    is_transient(error) && !error.contains_label(mongodb::error::UNKNOWN_TRANSACTION_COMMIT_RESULT)
}

#[cfg(test)]
pub(crate) mod command_test_support;
#[cfg(test)]
mod fence_error_tests;
