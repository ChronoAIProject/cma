//! Read-only observation custody; never grants native or lifecycle effects.
mod lease;
mod output_generation;
use super::sandbox_mongo::MongoSandboxStore;
use futures_util::TryStreamExt;
use layer2_core::{
    agent_ownership::{Member, OwnershipCoordinator},
    resource_access::ResourceAccessStore,
    sandbox::{agent_observation::*, AgentReadError},
    AgentId,
};
use mongodb::{
    bson::{doc, Document},
    Collection,
};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio::time::Instant;

pub(crate) const COLLECTION: &str = "agent_read_observers_v1";
const LEASE_MS: i64 = 15_000;

pub(crate) struct MongoAgentObservation {
    source: Arc<MongoSandboxStore>,
    leases: Collection<Document>,
    coordinator: Arc<OwnershipCoordinator>,
}

pub(crate) struct ObservationDestination {
    pub member: Member,
    pub authority: Arc<AgentObservationAuthority>,
}

impl MongoAgentObservation {
    pub(crate) async fn open(
        source: Arc<MongoSandboxStore>,
        coordinator: Arc<OwnershipCoordinator>,
    ) -> Result<Arc<Self>, AgentReadError> {
        let database = source.agents.namespace().db;
        let database = source.client.database(&database);
        super::schema::initialize_collections(&database, &[COLLECTION])
            .await
            .map_err(unavailable)?;
        // The implicit unique _id index is the only access path. Epoch rows
        // retain their watermark; correctness never relies on TTL cleanup.
        Ok(Arc::new(Self {
            leases: super::durability::majority_collection(&database, COLLECTION),
            source,
            coordinator,
        }))
    }

    pub(crate) async fn destination(
        &self,
        id: &AgentId,
        created_at: &str,
    ) -> Result<ObservationDestination, AgentReadError> {
        let started = Instant::now();
        if !self.coordinator.is_ready() {
            return Err(unavailable("member unavailable"));
        }
        let membership = layer2_core::agent_ownership::bounded(self.coordinator.store.membership())
            .await
            .map_err(unavailable)?;
        membership.require_compatible().map_err(unavailable)?;
        if let Some(row) = self
            .leases
            .find_one(doc! {"_id": id.as_str()})
            .await
            .map_err(unavailable)?
        {
            let retained = lease::read(&row)?;
            if retained.expires > membership.now_ms {
                return lease::destination(retained, created_at, &membership, started);
            }
        }
        // Reuse the native owner's placement when it exists, but do not acquire
        // or renew a native effect lease merely to read a stopped Agent's log.
        let native = layer2_core::agent_ownership::bounded(self.coordinator.store.owner(id))
            .await
            .map_err(unavailable)?;
        let selected = native
            .as_ref()
            .and_then(|owner| {
                membership.members.iter().find(|member| {
                    member.instance_id == owner.token.instance_id
                        && member.is_live(membership.now_ms)
                        && !member.draining
                })
            })
            .or_else(|| {
                layer2_core::agent_ownership::candidates(id, &membership, None)
                    .ok()
                    .and_then(|candidates| candidates.first().copied())
            })
            .ok_or_else(|| unavailable("no eligible observation member"))?;
        if let Err(error) = self.leases.update_one(doc! {"_id": id.as_str()}, doc! {"$setOnInsert": {
            "document_schema_version": 1_i64, "created_at": created_at, "instance_id": &selected.instance_id,
            "epoch": 0_i64, "expires_at": mongodb::bson::DateTime::from_millis(0),
            "next_refresh_at": mongodb::bson::DateTime::from_millis(0), "round": ""
        }}).upsert(true).await {
            if !super::sandbox_mongo::is_duplicate_key(&error) { return Err(unavailable(error)); }
        }
        let row = self.leases.find_one_and_update(
            doc! {"_id": id.as_str(), "document_schema_version": 1_i64, "epoch": {"$gte": 0_i64, "$lt": i64::MAX}, "$expr": {"$lte": ["$expires_at", "$$NOW"]}},
            vec![doc! {"$set": {
                "document_schema_version": 1_i64,
                "created_at": created_at,
                "instance_id": &selected.instance_id,
                "epoch": {"$add": [{"$ifNull": ["$epoch", 0_i64]}, 1_i64]},
                "expires_at": {"$add": ["$$NOW", LEASE_MS]},
                "next_refresh_at": {"$max": [{"$ifNull": ["$next_refresh_at", "$$NOW"]}, {"$add": ["$$NOW", 1000_i64]}]},
                "round": "",
            }}],
        ).return_document(mongodb::options::ReturnDocument::After).await.map_err(unavailable)?;
        let row = match row {
            Some(row) => row,
            None => self
                .leases
                .find_one(doc! {"_id": id.as_str()})
                .await
                .map_err(unavailable)?
                .ok_or_else(|| unavailable("observation acquisition absent"))?,
        };
        lease::destination(lease::read(&row)?, created_at, &membership, started)
    }

    async fn moved_native_owners(
        &self,
        ids: &[AgentId],
    ) -> Result<std::collections::BTreeSet<String>, AgentReadError> {
        if ids.is_empty() {
            return Ok(Default::default());
        }
        let database = self
            .source
            .client
            .database(&self.source.agents.namespace().db);
        let rows: Vec<Document> = database
            .collection(super::agent_ownership::OWNERS)
            .find(doc! {
                "_id": {"$in": ids.iter().map(AgentId::as_str).collect::<Vec<_>>()},
                "instance_id": {"$ne": &self.coordinator.instance_id},
                "$expr": {"$gt": ["$expires_at", "$$NOW"]},
            })
            .projection(doc! {"_id": 1})
            .batch_size(ids.len() as u32 + 1)
            .await
            .map_err(unavailable)?
            .try_collect()
            .await
            .map_err(unavailable)?;
        rows.into_iter()
            .map(|row| row.get_str("_id").map(str::to_owned).map_err(unavailable))
            .collect()
    }

    async fn refresh_inner(
        &self,
        tokens: &[AgentObservationToken],
    ) -> Result<Vec<AgentObservationRefresh>, AgentReadError> {
        if tokens.len() > MAX_OBSERVATION_COHORT || !self.coordinator.is_ready() {
            return Err(unavailable("invalid observation cohort"));
        }
        if tokens.is_empty() {
            return Ok(Vec::new());
        }
        if tokens
            .iter()
            .any(|token| token.instance_id != self.coordinator.instance_id)
        {
            return Err(unavailable("observation belongs to a peer"));
        }
        let started = Instant::now();
        let round = uuid::Uuid::new_v4().to_string();
        let filters: Vec<_> = tokens.iter().map(lease::filter).collect::<Result<_, _>>()?;
        self.leases
            .update_many(
                doc! {"$or": &filters, "$expr": {"$and": [
                    {"$gt": ["$expires_at", "$$NOW"]}, {"$lte": ["$next_refresh_at", "$$NOW"]}
                ]}},
                vec![doc! {"$set": {
                    "expires_at": {"$add": ["$$NOW", LEASE_MS]},
                    "next_refresh_at": {"$add": ["$$NOW", 1000_i64]}, "round": &round,
                }}],
            )
            .await
            .map_err(unavailable)?;
        let rows: Vec<Document> = self
            .leases
            .aggregate([
                doc! {"$match": {"$or": filters}},
                doc! {"$set": {"observed_at": "$$NOW"}},
            ])
            .batch_size(tokens.len() as u32 + 1)
            .await
            .map_err(unavailable)?
            .try_collect()
            .await
            .map_err(unavailable)?;
        let mut retained = BTreeMap::new();
        for row in rows {
            let now = row
                .get_datetime("observed_at")
                .map_err(unavailable)?
                .timestamp_millis();
            let value = lease::read(&row)?;
            let claimed = row.get_str("round").map_err(unavailable)? == round;
            let id = value.token.agent_id.to_string();
            if retained.insert(id, (value, now, claimed)).is_some() {
                return Err(unavailable("duplicate read custody"));
            }
        }
        let ids: Vec<_> = tokens
            .iter()
            .filter(|token| {
                retained
                    .get(token.agent_id.as_str())
                    .is_some_and(|(lease, now, claimed)| {
                        *claimed && lease.token == **token && lease.expires > *now
                    })
            })
            .map(|token| token.agent_id.clone())
            .collect();
        if !self.coordinator.is_ready() {
            return Err(unavailable("member expired during observation"));
        }
        let moved = self.moved_native_owners(&ids).await?;
        let snapshots = self
            .source
            .agent_observation_snapshots(&ids)
            .await
            .map_err(unavailable)?;
        if snapshots.len() != ids.len() {
            return Err(unavailable("incomplete observation cohort"));
        }
        let mut snapshots: BTreeMap<_, _> = ids
            .into_iter()
            .map(|id| id.to_string())
            .zip(snapshots)
            .collect();
        tokens
            .iter()
            .map(|token| {
                let Some((lease, now, claimed)) = retained.remove(token.agent_id.as_str()) else {
                    return Ok(AgentObservationRefresh {
                        token: token.clone(),
                        until: started,
                        snapshot: None,
                    });
                };
                let remaining = lease.expires.saturating_sub(now).max(0) as u64;
                let snapshot = if moved.contains(token.agent_id.as_str()) {
                    Some(Err(unavailable("native placement changed; reconnect")))
                } else if claimed {
                    Some(
                        snapshots
                            .remove(token.agent_id.as_str())
                            .ok_or_else(|| unavailable("claimed snapshot absent"))?
                            .map_err(|error| AgentReadError::Access(error.into())),
                    )
                } else {
                    None
                };
                Ok(AgentObservationRefresh {
                    token: lease.token,
                    until: started + Duration::from_millis(remaining),
                    snapshot,
                })
            })
            .collect()
    }
}

#[async_trait::async_trait]
impl AgentObservationStore for MongoAgentObservation {
    fn ready(&self) -> bool {
        self.coordinator.is_ready()
    }
    async fn commands(
        &self,
        ids: &[layer2_core::ids::AgentResponseId],
    ) -> Result<
        Vec<Result<layer2_core::sandbox::agent_io::AdmittedAgentCommand, AgentReadError>>,
        AgentReadError,
    > {
        if ids.is_empty() || ids.len() > MAX_OBSERVATION_COHORT || !self.coordinator.is_ready() {
            return Err(unavailable("invalid command cohort"));
        }
        let ids_bson: Vec<_> = ids.iter().map(|id| id.as_str()).collect();
        let mut rows = self
            .source
            .agent_commands
            .find(doc! {"response_id":{"$in":ids_bson}})
            .limit(ids.len() as i64 + 1)
            .batch_size(ids.len() as u32 + 1)
            .max_time(Duration::from_secs(2))
            .await
            .map_err(unavailable)?;
        let mut records = BTreeMap::new();
        let mut bytes = 0usize;
        while let Some(row) = rows.try_next().await.map_err(unavailable)? {
            bytes = bytes.saturating_add(row.to_vec().map_err(unavailable)?.len());
            if bytes > MAX_PAGE_BYTES {
                return Err(unavailable("command observation byte budget"));
            }
            let record = super::agent_commands::records::read(&row).map_err(unavailable)?;
            let id = record
                .receipt
                .response_id
                .clone()
                .ok_or_else(|| unavailable("command response absent"))?;
            if records.insert(id, record).is_some() {
                return Err(unavailable("duplicate response command"));
            }
        }
        Ok(ids
            .iter()
            .map(|id| records.get(id).cloned().ok_or(AgentReadError::NotFound))
            .collect())
    }
    async fn source_generation(
        &self,
        agent: &layer2_core::sandbox::Agent,
    ) -> Result<Option<Vec<u8>>, layer2_core::sandbox::store::SandboxStoreError> {
        use layer2_core::sandbox::store::SandboxStoreError;
        use sha2::{Digest, Sha256};
        let row = self
            .source
            .agents
            .find_one(doc! {"_id": agent.id.as_str()})
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
            .ok_or(SandboxStoreError::NotFound)?;
        if super::agents::read_agent(&row)? != *agent {
            return Ok(None);
        }
        // Exact retained metadata includes format, source/proof generations and
        // quarantine. No cached result survives a metadata change or adoption.
        Ok(Some(
            Sha256::digest(
                row.to_vec()
                    .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?,
            )
            .to_vec(),
        ))
    }
    async fn output_generation(
        &self,
        agent: &layer2_core::sandbox::Agent,
    ) -> Result<Option<Vec<u8>>, layer2_core::sandbox::store::SandboxStoreError> {
        output_generation::read(&self.source, agent).await
    }
    async fn fresh(
        &self,
        tokens: &[AgentObservationToken],
    ) -> Result<
        Vec<Result<layer2_core::resource_access::ResourceAccessSnapshot, AgentReadError>>,
        AgentReadError,
    > {
        if tokens.len() > MAX_OBSERVATION_COHORT || !self.coordinator.is_ready() {
            return Err(unavailable("invalid release cohort"));
        }
        if tokens.is_empty() {
            return Ok(Vec::new());
        }
        let filters: Vec<_> = tokens.iter().map(lease::filter).collect::<Result<_, _>>()?;
        let rows: Vec<Document> = self
            .leases
            .find(doc! {"$or": filters, "$expr": {"$gt": ["$expires_at", "$$NOW"]}})
            .batch_size(tokens.len() as u32 + 1)
            .await
            .map_err(unavailable)?
            .try_collect()
            .await
            .map_err(unavailable)?;
        let current: BTreeMap<_, _> = rows
            .iter()
            .map(|row| {
                lease::read(row).map(|lease| (lease.token.agent_id.to_string(), lease.token))
            })
            .collect::<Result<_, _>>()?;
        let ids: Vec<_> = tokens
            .iter()
            .filter(|token| {
                token.instance_id == self.coordinator.instance_id
                    && current.get(token.agent_id.as_str()) == Some(token)
            })
            .map(|token| token.agent_id.clone())
            .collect();
        let moved = self.moved_native_owners(&ids).await?;
        let snapshots = self
            .source
            .agent_observation_snapshots(&ids)
            .await
            .map_err(unavailable)?;
        let mut snapshots: BTreeMap<_, _> = ids
            .into_iter()
            .map(|id| id.to_string())
            .zip(snapshots)
            .collect();
        if !self.coordinator.is_ready() {
            return Err(unavailable("member expired before release"));
        }
        Ok(tokens
            .iter()
            .map(|token| {
                if moved.contains(token.agent_id.as_str()) {
                    return Err(unavailable("native placement changed; reconnect"));
                }
                let snapshot = snapshots
                    .remove(token.agent_id.as_str())
                    .ok_or_else(|| unavailable("read epoch changed"))?
                    .map_err(|error| AgentReadError::Access(error.into()))?;
                if snapshot
                    .lineage
                    .agent
                    .as_ref()
                    .is_none_or(|agent| agent.created_at != token.created_at)
                {
                    return Err(AgentReadError::NotFound);
                }
                Ok(snapshot)
            })
            .collect())
    }

    async fn refresh(
        &self,
        tokens: &[AgentObservationToken],
    ) -> Result<Vec<AgentObservationRefresh>, AgentReadError> {
        tokio::time::timeout(Duration::from_secs(2), self.refresh_inner(tokens))
            .await
            .map_err(|_| unavailable("observation timed out"))?
    }
}

fn unavailable(error: impl std::fmt::Display) -> AgentReadError {
    AgentReadError::Unavailable(format!("read observation: {error}"))
}
