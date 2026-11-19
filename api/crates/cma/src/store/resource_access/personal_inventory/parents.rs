//! Explicit parent filters and final Agent fences share bounded snapshot reads.
use super::*;
use crate::store::resource_access::{records, snapshot_lineage};
use layer2_core::resource_access::{PersonalAgentInventoryFence, ResourceTarget};

pub(in crate::store) async fn fence(
    store: &MongoSandboxStore,
    agents: &[Agent],
    parents: &[ResourceTarget],
) -> Result<PersonalAgentInventoryFence, ResourceAccessStoreError> {
    if parents.len() > 2
        || parents.iter().any(|target| {
            !matches!(
                target,
                ResourceTarget::Sandbox { .. } | ResourceTarget::Workspace { .. }
            )
        })
        || agents.len() > usize::from(MAX_PAGE_SIZE) + 1
        || agents
            .windows(2)
            .any(|pair| pair[0].id.as_str() >= pair[1].id.as_str())
    {
        return Err(ResourceAccessStoreError::Invalid);
    }
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .await
        .map_err(|_| ResourceAccessStoreError::Unavailable)?;
    let mut sources = Sources {
        lineage: vec![Vec::new(); parents.len()],
        shares: vec![Vec::new(); parents.len()],
    };
    let current = read_with_sources(
        store,
        agents,
        &mut session,
        pipeline(store, parents),
        |kind, document| sources.insert(kind, document),
    )
    .await
    .map_err(error)?;
    if current.len() != agents.len()
        || current
            .iter()
            .zip(agents)
            .any(|(row, selected)| row.lineage.agent.as_ref() != Some(selected))
    {
        return Err(ResourceAccessStoreError::Conflict);
    }
    let mut snapshots = Vec::new();
    for ((target, lineage), shares) in parents.iter().zip(sources.lineage).zip(sources.shares) {
        let (lineage, policy) = snapshot_lineage::decode(target, lineage)?;
        let shares = shares
            .iter()
            .map(|document| records::read_share(&lineage.sandbox.id, document))
            .collect::<Result<Vec<_>, _>>()?;
        let snapshot = ResourceAccessSnapshot {
            lineage,
            policy,
            shares,
        };
        snapshot.scope(target)?;
        snapshots.push(snapshot);
    }
    Ok(PersonalAgentInventoryFence {
        agents: current,
        parents: snapshots,
    })
}

fn pipeline(store: &MongoSandboxStore, parents: &[ResourceTarget]) -> Vec<Document> {
    let mut result = Vec::new();
    for (index, target) in parents.iter().enumerate() {
        let (collection, mut branch) = snapshot_lineage::plan(store, target);
        branch.extend(query::frame(&format!("parent_{index}_lineage")));
        result.push(doc! {"$unionWith":{"coll":collection.name(),"pipeline":branch}});
        let (collection, id, parent) = match target {
            ResourceTarget::Sandbox { sandbox_id } => {
                (&store.sandboxes, sandbox_id.as_str(), "$_id")
            }
            ResourceTarget::Workspace { workspace_id } => (
                &store.workspaces,
                workspace_id.as_str(),
                "$record.sandbox_id",
            ),
            _ => unreachable!("parent targets validated before query construction"),
        };
        let mut branch = vec![
            doc! {"$match":{"_id":id}},
            doc! {"$project":{"_id":0,"parent":parent}},
            doc! {"$lookup":{
                "from":store.resource_shares.name(),"localField":"parent","foreignField":"sandbox_id","as":"shares","let":{"parent":"$parent"},
                "pipeline":[{"$match":{"revoked":false,"selector_kind":"sandbox","$expr":{"$cond":[{"$isArray":"$selector_id"},{"$in":["$$parent","$selector_id"]},{"$eq":["$selector_id","$$parent"]}]}}},{"$sort":{"_id":1}},{"$limit":MAX_ACTIVE_RESOURCE_SHARES as i64 + 1}]
            }},
            doc! {"$unwind":"$shares"},
            doc! {"$replaceWith":"$shares"},
        ];
        branch.extend(query::frame(&format!("parent_{index}_shares")));
        result.push(doc! {"$unionWith":{"coll":collection.name(),"pipeline":branch}});
    }
    result
}

struct Sources {
    lineage: Vec<Vec<Document>>,
    shares: Vec<Vec<Document>>,
}
impl Sources {
    fn insert(&mut self, kind: &str, document: Document) -> Result<(), ResourceAccessStoreError> {
        let parts = kind.split('_').collect::<Vec<_>>();
        let ["parent", index, kind] = parts.as_slice() else {
            return Err(ResourceAccessStoreError::Corrupt);
        };
        let index = index
            .parse::<usize>()
            .map_err(|_| ResourceAccessStoreError::Corrupt)?;
        let (source, limit) = match *kind {
            "lineage" => (&mut self.lineage, 6),
            "shares" => (&mut self.shares, MAX_ACTIVE_RESOURCE_SHARES + 1),
            _ => return Err(ResourceAccessStoreError::Corrupt),
        };
        let rows = source
            .get_mut(index)
            .ok_or(ResourceAccessStoreError::Corrupt)?;
        rows.push(document);
        if rows.len() > limit {
            return Err(ResourceAccessStoreError::Corrupt);
        }
        Ok(())
    }
}
