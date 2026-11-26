//! Fetch validated ancestry in one consistent snapshot without combining output rows.
use layer2_core::resource_access::{
    ResourceAccessStoreError, ResourceLineage, ResourcePolicyRecord, ResourceTarget,
};
use mongodb::bson::{doc, Document};
use mongodb::{ClientSession, Collection};

use super::super::{
    agents::read_agent, sandbox_mongo::MongoSandboxStore, sandboxes::read_sandbox,
    workspaces::read_workspace, worktrees::read_worktree,
};
use super::{records, ReadError};

const FIELDS: [&str; 6] = [
    "agent",
    "worktree",
    "workspace",
    "sandbox",
    "assigned",
    "policy",
];

pub(in crate::store) struct LineageCollections<'a> {
    pub agents: &'a Collection<Document>,
    pub worktrees: &'a Collection<Document>,
    pub workspaces: &'a Collection<Document>,
    pub sandboxes: &'a Collection<Document>,
    pub resource_policies: &'a Collection<Document>,
}

impl<'a> From<&'a MongoSandboxStore> for LineageCollections<'a> {
    fn from(store: &'a MongoSandboxStore) -> Self {
        Self {
            agents: &store.agents,
            worktrees: &store.worktrees,
            workspaces: &store.workspaces,
            sandboxes: &store.sandboxes,
            resource_policies: &store.resource_policies,
        }
    }
}

pub(super) async fn read(
    store: &MongoSandboxStore,
    target: &ResourceTarget,
    session: &mut ClientSession,
    snapshot_session: bool,
) -> Result<(ResourceLineage, Option<ResourcePolicyRecord>), ReadError> {
    read_from(store.into(), target, session, snapshot_session).await
}

pub(in crate::store) async fn read_from(
    collections: LineageCollections<'_>,
    target: &ResourceTarget,
    session: &mut ClientSession,
    snapshot_session: bool,
) -> Result<(ResourceLineage, Option<ResourcePolicyRecord>), ReadError> {
    let (collection, pipeline) = plan_from(collections, target);
    let collection = super::super::snapshot_collection::collection(collection, snapshot_session);
    let mut cursor = collection
        .aggregate(pipeline)
        .batch_size(FIELDS.len() as u32 + 1)
        .session(&mut *session)
        .await?;
    let mut documents = Vec::with_capacity(FIELDS.len());
    while cursor.advance(&mut *session).await? {
        documents.push(cursor.deserialize_current()?);
        if documents.len() > FIELDS.len() {
            return Err(ResourceAccessStoreError::Corrupt.into());
        }
    }
    decode(target, documents).map_err(Into::into)
}

pub(super) fn decode(
    target: &ResourceTarget,
    documents: Vec<Document>,
) -> Result<(ResourceLineage, Option<ResourcePolicyRecord>), ResourceAccessStoreError> {
    if documents.is_empty() {
        return Err(ResourceAccessStoreError::NotFound);
    }
    let [agent, worktree, workspace, sandbox, assigned, policy]: [Document; 6] = documents
        .try_into()
        .map_err(|_| ResourceAccessStoreError::Corrupt)?;
    // Keep the existing complete record readers, including their indexed-field
    // agreement checks. A join result alone never establishes valid lineage.
    let agent = matches!(target, ResourceTarget::Agent { .. })
        .then(|| read_agent(&agent))
        .transpose()
        .map_err(|_| ResourceAccessStoreError::Corrupt)?;
    let worktree = matches!(
        target,
        ResourceTarget::Agent { .. } | ResourceTarget::Worktree { .. }
    )
    .then(|| read_worktree(&worktree))
    .transpose()
    .map_err(|_| ResourceAccessStoreError::Corrupt)?;
    let workspace = (!matches!(target, ResourceTarget::Sandbox { .. }))
        .then(|| read_workspace(&workspace))
        .transpose()
        .map_err(|_| ResourceAccessStoreError::Corrupt)?;
    let sandbox = read_sandbox(&sandbox).map_err(|_| ResourceAccessStoreError::Corrupt)?;
    let lineage = ResourceLineage {
        sandbox,
        workspace,
        worktree,
        agent,
    };
    lineage.validate(target)?;
    if matches!(target, ResourceTarget::Worktree { .. })
        && lineage
            .worktree
            .as_ref()
            .is_some_and(|tree| tree.agent_id.is_some())
    {
        let assigned = read_agent(&assigned).map_err(|_| ResourceAccessStoreError::Corrupt)?;
        lineage.validate_worktree_agent(&assigned)?;
    }
    let policy = (!policy.is_empty())
        .then(|| records::read_policy(&policy))
        .transpose()?;
    Ok((lineage, policy))
}

pub(super) fn plan<'a>(
    store: &'a MongoSandboxStore,
    target: &ResourceTarget,
) -> (&'a Collection<Document>, Vec<Document>) {
    plan_from(store.into(), target)
}

fn plan_from<'a>(
    store: LineageCollections<'a>,
    target: &ResourceTarget,
) -> (&'a Collection<Document>, Vec<Document>) {
    let (collection, id, root) = match target {
        ResourceTarget::Agent { agent_id } => (store.agents, agent_id.as_str(), "agent"),
        ResourceTarget::Worktree { worktree_id } => {
            (store.worktrees, worktree_id.as_str(), "worktree")
        }
        ResourceTarget::Workspace { workspace_id } => {
            (store.workspaces, workspace_id.as_str(), "workspace")
        }
        ResourceTarget::Sandbox { sandbox_id } => (store.sandboxes, sandbox_id.as_str(), "sandbox"),
    };
    let mut wrapped = Document::new();
    wrapped.insert(root, "$$ROOT");
    let mut pipeline = vec![doc! {"$match":{"_id":id}}, doc! {"$replaceWith":wrapped}];
    if matches!(target, ResourceTarget::Agent { .. }) {
        join(
            &mut pipeline,
            store.worktrees,
            "agent.record.worktree_id",
            "worktree",
        );
    }
    if matches!(
        target,
        ResourceTarget::Agent { .. } | ResourceTarget::Worktree { .. }
    ) {
        join(
            &mut pipeline,
            store.workspaces,
            "worktree.record.workspace_id",
            "workspace",
        );
    }
    if !matches!(target, ResourceTarget::Sandbox { .. }) {
        join(
            &mut pipeline,
            store.sandboxes,
            "workspace.record.sandbox_id",
            "sandbox",
        );
    }
    if matches!(target, ResourceTarget::Worktree { .. }) {
        join(
            &mut pipeline,
            store.agents,
            "worktree.record.agent_id",
            "assigned",
        );
    }
    join(
        &mut pipeline,
        store.resource_policies,
        "sandbox._id",
        "policy",
    );
    // Intermediate aggregation documents may exceed the BSON output limit.
    // Emit each unchanged source record separately, so legal retained ancestry
    // is not required to fit into a single combined 16 MiB result document.
    let documents: Vec<Document> = FIELDS
        .iter()
        .map(|field| doc! {"$ifNull":[format!("${field}"), {}]})
        .collect();
    pipeline.extend([
        doc! {"$project":{"_id":0,"records":documents}},
        doc! {"$unwind":"$records"},
        doc! {"$replaceWith":"$records"},
    ]);
    (collection, pipeline)
}

fn join(pipeline: &mut Vec<Document>, collection: &Collection<Document>, local: &str, field: &str) {
    pipeline.push(doc! {"$lookup":{
        "from":collection.name(), "localField":local, "foreignField":"_id",
        // Valid references are scalar IDs; malformed arrays must not fan out.
        "pipeline":[{"$limit":1}], "as":field,
    }});
    let mut flatten = Document::new();
    flatten.insert(
        field,
        doc! {"$ifNull":[{"$arrayElemAt":[format!("${field}"),0]},{}]},
    );
    pipeline.push(doc! {"$set":flatten});
}
