//! Majority-backed membership and permanent, monotonically fenced owner rows.

use futures_util::TryStreamExt;
use layer2_core::agent_ownership::{
    Member, Membership, OwnershipError, OwnershipLease, OwnershipStore, OwnershipToken,
    DEFAULT_LEASE_MS, PROTOCOL_VERSION,
};
use layer2_core::AgentId;
use mongodb::bson::{doc, Document};
use mongodb::{Client, ClientSession, Collection, Database};

use super::durability::majority_collection;
use super::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, start_transaction, CommitOutcome,
    TRANSACTION_ATTEMPTS,
};

mod acquisition;
mod effects;
mod membership;
mod records;
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
}

impl MongoAgentOwnership {
    pub(crate) async fn open(client: &Client, name: &str) -> Result<Self, OwnershipError> {
        let database = client.database(name);
        super::schema::initialize_collections(&database, &[MEMBERS, OWNERS])
            .await
            .map_err(unavailable)?;
        let store = Self {
            client: client.clone(),
            members: majority_collection(&database, MEMBERS),
            owners: majority_collection(&database, OWNERS),
            database,
        };
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

    async fn server_now(&self) -> Result<i64, OwnershipError> {
        self.database
            .run_command(doc! {"hello": 1})
            .await
            .map_err(unavailable)?
            .get_datetime("localTime")
            .map(|time| time.timestamp_millis())
            .map_err(invalid)
    }

    /// A write in the effect's own transaction creates a conflict with takeover.
    /// A preceding read, even a majority read, would leave a TOCTOU gap.
    pub(crate) async fn fence_transaction(
        &self,
        token: &OwnershipToken,
        session: &mut ClientSession,
    ) -> Result<(), OwnershipError> {
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
            .map_err(unavailable)?;
        if member.matched_count != 1 {
            return Err(OwnershipError::Stale);
        }
        let matched = self
            .owners
            .update_one(
                token_filter(token)?,
                doc! {"$inc": {"effect_revision": 1_i64}},
            )
            .session(&mut *session)
            .await
            .map_err(unavailable)?
            .matched_count;
        if matched == 1 {
            Ok(())
        } else {
            Err(OwnershipError::Stale)
        }
    }
}

#[async_trait::async_trait]
impl OwnershipStore for MongoAgentOwnership {
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

    async fn renew(&self, token: &OwnershipToken) -> Result<OwnershipLease, OwnershipError> {
        self.renew_owner(token).await
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
}

fn unavailable(error: impl std::fmt::Display) -> OwnershipError {
    OwnershipError::Unavailable(error.to_string())
}

fn invalid(error: impl std::fmt::Display) -> OwnershipError {
    OwnershipError::Invalid(error.to_string())
}

#[cfg(test)]
mod mongo_tests;
