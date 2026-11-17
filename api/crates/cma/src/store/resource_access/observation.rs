//! One fresh snapshot for independent readers of a bounded Agent cohort.
use super::*;
use layer2_core::{AgentId, SandboxId};
use mongodb::{bson::doc, ClientSession};
use std::collections::{BTreeMap, BTreeSet};

type ResultRow = Result<ResourceAccessSnapshot, ResourceAccessStoreError>;
const MAX_FACT_BYTES: usize = 64 * 1024 * 1024;

pub(super) async fn read(
    store: &MongoSandboxStore,
    agents: &[AgentId],
) -> Result<Vec<ResultRow>, ResourceAccessStoreError> {
    if agents.len() > 100 {
        return Err(ResourceAccessStoreError::Capacity);
    }
    if agents.is_empty() {
        return Ok(Vec::new());
    }
    for _ in 0..TRANSACTION_ATTEMPTS {
        let mut session = store
            .client
            .start_session()
            .snapshot(true)
            .await
            .map_err(|_| ResourceAccessStoreError::Unavailable)?;
        match read_once(store, agents, &mut session).await {
            Ok(rows) => return Ok(rows),
            Err(ReadError::Domain(error)) => return Err(error),
            Err(ReadError::Driver(error)) if is_transient(&error) => continue,
            Err(ReadError::Driver(_)) => return Err(ResourceAccessStoreError::Unavailable),
        }
    }
    Err(ResourceAccessStoreError::Unavailable)
}

async fn read_once(
    store: &MongoSandboxStore,
    agents: &[AgentId],
    session: &mut ClientSession,
) -> Result<Vec<ResultRow>, ReadError> {
    let ids: BTreeSet<_> = agents.iter().map(AgentId::as_str).collect();
    let first = ResourceTarget::Agent {
        agent_id: agents[0].clone(),
    };
    let (_, mut pipeline) = snapshot_lineage::plan(store, &first);
    pipeline[0] = doc! {"$match": {"_id": {"$in": ids.iter().copied().collect::<Vec<_>>()}}};
    // The existing plan emits six original BSON records per Agent. Keep that
    // representation: joining legal large ancestors into one row can exceed
    // Mongo's document limit. No caller-visible aggregate is inferred here.
    let collection = super::super::snapshot_collection::collection(&store.agents, true);
    let mut cursor = collection
        .aggregate(pipeline)
        .batch_size((ids.len() * 6 + 1) as u32)
        .max_time(std::time::Duration::from_secs(10))
        .session(&mut *session)
        .await?;
    let mut rows = BTreeMap::new();
    let mut group = Vec::with_capacity(6);
    let mut bytes = 0;
    while cursor.advance(&mut *session).await? {
        let document = cursor.deserialize_current()?;
        charge(&document, &mut bytes)?;
        group.push(document);
        if group.len() != 6 {
            continue;
        }
        let id = group[0]
            .get_str("_id")
            .map_err(|_| ResourceAccessStoreError::Corrupt)?
            .to_owned();
        if !ids.contains(id.as_str()) || rows.contains_key(&id) {
            return Err(ResourceAccessStoreError::Corrupt.into());
        }
        let target = ResourceTarget::Agent {
            agent_id: AgentId(id.clone()),
        };
        let row = snapshot_lineage::decode(&target, std::mem::take(&mut group)).map(
            |(lineage, policy)| ResourceAccessSnapshot {
                lineage,
                policy,
                shares: Vec::new(),
            },
        );
        rows.insert(id, row);
    }
    if !group.is_empty() {
        return Err(ResourceAccessStoreError::Corrupt.into());
    }
    let roots: BTreeSet<_> = rows
        .values()
        .filter_map(|row| row.as_ref().ok())
        .map(|row| row.lineage.sandbox.id.as_str())
        .collect();
    if !roots.is_empty() {
        let collection =
            super::super::snapshot_collection::collection(&store.resource_shares, true);
        let mut cursor = collection
            .find(doc! {"revoked": false, "$or": [
                {"selector_kind": "sandbox", "selector_id": {"$in": roots.iter().copied().collect::<Vec<_>>()}},
                {"selector_kind": "agent", "selector_id": {"$in": ids.iter().copied().collect::<Vec<_>>()}}
            ]})
            .sort(doc! {"_id": 1})
            .batch_size(1000)
            .limit((ids.len() * (layer2_core::resource_access::MAX_ACTIVE_RESOURCE_SHARES + 1)) as i64)
            .max_time(std::time::Duration::from_secs(10))
            .session(&mut *session)
            .await?;
        let mut shares = Vec::new();
        while cursor.advance(&mut *session).await? {
            let document = cursor.deserialize_current()?;
            charge(&document, &mut bytes)?;
            let sandbox = document
                .get_str("sandbox_id")
                .map_err(|_| ResourceAccessStoreError::Corrupt)?;
            let sandbox = SandboxId(sandbox.into());
            shares.push((sandbox.clone(), records::read_share(&sandbox, &document)?));
            if shares.len()
                >= ids.len() * (layer2_core::resource_access::MAX_ACTIVE_RESOURCE_SHARES + 1)
            {
                return Err(ResourceAccessStoreError::Capacity.into());
            }
        }
        for (id, row) in &mut rows {
            let Ok(snapshot) = row else { continue };
            let target = ResourceTarget::Agent {
                agent_id: AgentId(id.clone()),
            };
            let scope = match snapshot.scope(&target) {
                Ok(scope) => scope,
                Err(error) => {
                    *row = Err(error);
                    continue;
                }
            };
            snapshot.shares = shares
                .iter()
                .filter(|(sandbox, share)| {
                    *sandbox == scope.sandbox_id && share.selector.covers(&scope)
                })
                .map(|(_, share)| share.clone())
                .collect();
            if let Err(error) = snapshot.scope(&target) {
                *row = Err(error);
            }
        }
    }
    Ok(agents
        .iter()
        .map(|id| {
            rows.get(id.as_str())
                .cloned()
                .unwrap_or(Err(ResourceAccessStoreError::NotFound))
        })
        .collect())
}

fn charge(document: &Document, bytes: &mut usize) -> Result<(), ReadError> {
    *bytes += document
        .to_vec()
        .map_err(|_| ResourceAccessStoreError::Corrupt)?
        .len();
    if *bytes > MAX_FACT_BYTES {
        return Err(ResourceAccessStoreError::Capacity.into());
    }
    Ok(())
}
