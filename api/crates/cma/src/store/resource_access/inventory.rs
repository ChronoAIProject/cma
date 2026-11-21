use std::{collections::BTreeMap, time::Duration};

use layer2_core::resource_access::{
    ResourceAccessSnapshot, ResourceAccessStoreError, ResourceTarget,
};
use layer2_core::sandbox::store::PageRequest;
use layer2_core::sandbox::{Agent, AgentListQuery};
use layer2_core::{AgentId, SubjectId};
use mongodb::{
    bson::{doc, Document},
    ClientSession,
};

use super::super::{
    agents::read_agent,
    sandbox_mongo::{record_bson, MongoSandboxStore, AGENTS},
    snapshot_collection,
};
use super::{personal_inventory, snapshot, ReadError};

pub(in crate::store) async fn read(
    store: &MongoSandboxStore,
    organization: &SubjectId,
    query: &AgentListQuery,
    page: &PageRequest,
    session: &mut ClientSession,
) -> Result<Vec<ResourceAccessSnapshot>, ReadError> {
    read_scope(
        store,
        Scope::Organization(organization),
        query,
        page,
        session,
        false,
    )
    .await
}

/// Only callers using explicit snapshot sessions may select union source reads.
pub(super) async fn read_batch(
    store: &MongoSandboxStore,
    organization: &SubjectId,
    query: &AgentListQuery,
    page: &PageRequest,
    session: &mut ClientSession,
) -> Result<Vec<ResourceAccessSnapshot>, ReadError> {
    read_scope(
        store,
        Scope::Organization(organization),
        query,
        page,
        session,
        true,
    )
    .await
}

pub(in crate::store) async fn read_shared(
    store: &MongoSandboxStore,
    recipient: &SubjectId,
    organizations: &[SubjectId],
    query: &AgentListQuery,
    page: &PageRequest,
    session: &mut ClientSession,
) -> Result<Vec<ResourceAccessSnapshot>, ReadError> {
    read_scope(
        store,
        Scope::Shared {
            recipient,
            organizations,
        },
        query,
        page,
        session,
        false,
    )
    .await
}

enum Scope<'a> {
    Organization(&'a SubjectId),
    Shared {
        recipient: &'a SubjectId,
        organizations: &'a [SubjectId],
    },
}

async fn read_scope(
    store: &MongoSandboxStore,
    scope: Scope<'_>,
    query: &AgentListQuery,
    page: &PageRequest,
    session: &mut ClientSession,
    batched: bool,
) -> Result<Vec<ResourceAccessSnapshot>, ReadError> {
    PageRequest::new(page.limit, page.cursor.clone())
        .map_err(|_| ResourceAccessStoreError::Invalid)?;
    let filter = agent_filter(query, page)?;
    let limit = i64::from(page.limit) + 1;
    let mut branches = Vec::new();
    let admission = match &scope {
        Scope::Organization(organization) => {
            branches.push((
                &store.resource_policies,
                doc! {"owner_kind":"organization","owner_subject":organization.as_str()},
                "_id",
                "sandbox_id",
            ));
            doc! {"target_org_id":organization.as_str(),"revoked":false}
        }
        Scope::Shared {
            recipient,
            organizations,
        } => {
            doc! {"$or":[
                {"target_org_id":{"$in":organizations.iter().map(SubjectId::as_str).collect::<Vec<_>>()}},
                {"accepted_subjects":recipient.as_str(), "$or":[
                    {"record.link.audience.kind":{"$in":["public","users"]}},
                    {"record.link.audience.organization_id":{"$in":organizations.iter().map(SubjectId::as_str).collect::<Vec<_>>()}}
                ]}
            ], "revoked":false,"$nor":[{"record.owner.kind":"person","record.owner.subject":recipient.as_str()}]}
        }
    };
    for (kind, foreign_field) in [("sandbox", "sandbox_id"), ("agent", "_id")] {
        let mut admission = admission.clone();
        admission.insert("selector_kind", kind);
        branches.push((
            &store.resource_shares,
            admission,
            "selector_id",
            foreign_field,
        ));
    }
    let mut candidates = BTreeMap::<String, Agent>::new();
    // Each indexed ownership/grant branch deduplicates and bounds its results
    // before transfer. Merging its first limit + 1 IDs is sufficient to find
    // the union's first limit + 1 IDs, even with arbitrary grant overlap.
    // Separate queries share one transaction on the legacy path. Batched reads
    // use one explicit snapshot session because `$unionWith` is not transaction-safe.
    for (collection, admission, local_field, foreign_field) in branches {
        let pipeline = vec![
            doc! {"$match":admission},
            doc! {"$lookup":{"from":AGENTS,"localField":local_field,"foreignField":foreign_field,"pipeline":[{"$match":filter.clone()}],"as":"agent"}},
            doc! {"$unwind":"$agent"},
            doc! {"$replaceWith":"$agent"},
            doc! {"$group":{"_id":"$_id","agent":{"$first":"$$ROOT"}}},
            doc! {"$replaceWith":"$agent"},
            doc! {"$sort":{"_id":1}},
            doc! {"$limit":limit},
        ];
        let collection = if batched {
            snapshot_collection::collection(collection, true)
        } else {
            collection.clone()
        };
        let mut cursor = collection
            .aggregate(pipeline)
            .max_time(Duration::from_secs(10))
            .session(&mut *session)
            .await?;
        while cursor.advance(&mut *session).await? {
            let agent = read_agent(&cursor.deserialize_current()?)
                .map_err(|_| ResourceAccessStoreError::Corrupt)?;
            if let Some(previous) = candidates.insert(agent.id.to_string(), agent.clone()) {
                if previous != agent {
                    return Err(ResourceAccessStoreError::Corrupt.into());
                }
            }
        }
    }
    let selected: Vec<_> = candidates.into_values().take(limit as usize).collect();
    let mut batch = if batched {
        let values = personal_inventory::read_captured_sources(
            store,
            &selected,
            session,
            Vec::new(),
            |_, _| Err(ResourceAccessStoreError::Corrupt),
        )
        .await?;
        if values.len() != selected.len() {
            return Err(ResourceAccessStoreError::Corrupt.into());
        }
        Some(values.into_iter())
    } else {
        None
    };
    let mut items = Vec::with_capacity(selected.len());
    for candidate in selected {
        let value = match &mut batch {
            Some(values) => values.next().ok_or(ResourceAccessStoreError::Corrupt)?,
            None => {
                snapshot::read(
                    store,
                    &ResourceTarget::Agent {
                        agent_id: AgentId(candidate.id.to_string()),
                    },
                    session,
                )
                .await?
            }
        };
        let agent = match &scope {
            Scope::Organization(organization) => value.organization_agent(organization)?,
            Scope::Shared {
                recipient,
                organizations,
            } => value.shared_agent(recipient, organizations)?,
        };
        if agent != &candidate {
            return Err(ResourceAccessStoreError::Corrupt.into());
        }
        items.push(value);
    }
    Ok(items)
}

fn agent_filter(
    query: &AgentListQuery,
    page: &PageRequest,
) -> Result<Document, ResourceAccessStoreError> {
    let mut filter = Document::new();
    if let Some(id) = &query.sandbox_id {
        filter.insert("sandbox_id", id.as_str());
    }
    if let Some(id) = &query.workspace_id {
        filter.insert("workspace_id", id.as_str());
    }
    if let Some(status) = query.status {
        filter.insert(
            "status",
            record_bson(&status).map_err(|_| ResourceAccessStoreError::Corrupt)?,
        );
    }
    if let Some(cursor) = &page.cursor {
        filter.insert("_id", doc! {"$gt":cursor});
    }
    Ok(filter)
}
