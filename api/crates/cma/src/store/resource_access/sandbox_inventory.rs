use super::super::{
    sandbox_mongo::{MongoSandboxStore, SANDBOXES},
    sandboxes::read_sandbox,
};
use super::{snapshot, ReadError};
use layer2_core::{
    resource_access::{ResourceAccessSnapshot, ResourceAccessStoreError, ResourceTarget},
    sandbox::{store::PageRequest, OwnerSandboxQuery, Sandbox},
    SandboxId, SubjectId,
};
use mongodb::{
    bson::{doc, Document},
    ClientSession,
};
use std::{collections::BTreeMap, time::Duration};

pub(super) async fn read(
    store: &MongoSandboxStore,
    organization: &SubjectId,
    query: &OwnerSandboxQuery,
    session: &mut ClientSession,
) -> Result<Vec<ResourceAccessSnapshot>, ReadError> {
    read_scope(store, Scope::Organization(organization), query, session).await
}

pub(super) async fn read_shared(
    store: &MongoSandboxStore,
    recipient: &SubjectId,
    organizations: &[SubjectId],
    query: &OwnerSandboxQuery,
    session: &mut ClientSession,
) -> Result<Vec<ResourceAccessSnapshot>, ReadError> {
    read_scope(
        store,
        Scope::Shared {
            recipient,
            organizations,
        },
        query,
        session,
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
    query: &OwnerSandboxQuery,
    session: &mut ClientSession,
) -> Result<Vec<ResourceAccessSnapshot>, ReadError> {
    PageRequest::new(query.page.limit, query.page.cursor.clone())
        .map_err(|_| ResourceAccessStoreError::Invalid)?;
    let mut filter = super::super::sandbox_quarantine::exclude(Document::new());
    if let Some(status) = query.status {
        filter.insert("status", status.name());
    }
    if query.exclude_retired {
        filter.insert("$and", vec![doc! { "status": { "$ne": "retired" } }]);
    }
    if let Some(cursor) = &query.page.cursor {
        filter.insert("_id", doc! {"$gt":cursor});
    }
    let limit = i64::from(query.page.limit) + 1;
    let mut branches = Vec::new();
    let admission = match &scope {
        Scope::Organization(organization) => {
            branches.push((
                &store.resource_policies,
                doc! {"owner_kind":"organization","owner_subject":organization.as_str()},
                "_id",
            ));
            doc! {"target_org_id":organization.as_str(),"revoked":false,"selector_kind":"sandbox"}
        }
        Scope::Shared {
            recipient,
            organizations,
        } => {
            if organizations.is_empty() {
                return Ok(Vec::new());
            }
            doc! {"target_org_id":{"$in":organizations.iter().map(SubjectId::as_str).collect::<Vec<_>>()},
            "revoked":false,"selector_kind":"sandbox",
            "$nor":[{"record.owner.kind":"person","record.owner.subject":recipient.as_str()}]}
        }
    };
    branches.push((&store.resource_shares, admission, "selector_id"));
    let mut candidates = BTreeMap::<String, Sandbox>::new();
    // Read both indexed branches in one transaction. Each returns a bounded,
    // deduplicated prefix; narrow Agent shares never enter this parent inventory.
    for (collection, admission, local_field) in branches {
        let pipeline = vec![
            doc! {"$match":admission},
            doc! {"$lookup":{"from":SANDBOXES,"localField":local_field,"foreignField":"_id","pipeline":[{"$match":filter.clone()}],"as":"sandbox"}},
            doc! {"$unwind":"$sandbox"},
            doc! {"$replaceWith":"$sandbox"},
            doc! {"$group":{"_id":"$_id","sandbox":{"$first":"$$ROOT"}}},
            doc! {"$replaceWith":"$sandbox"},
            doc! {"$sort":{"_id":1}},
            doc! {"$limit":limit},
        ];
        let mut cursor = collection
            .aggregate(pipeline)
            .max_time(Duration::from_secs(10))
            .session(&mut *session)
            .await?;
        while cursor.advance(&mut *session).await? {
            let sandbox = read_sandbox(&cursor.deserialize_current()?)
                .map_err(|_| ResourceAccessStoreError::Corrupt)?;
            if let Some(previous) = candidates.insert(sandbox.id.to_string(), sandbox.clone()) {
                if previous != sandbox {
                    return Err(ResourceAccessStoreError::Corrupt.into());
                }
            }
        }
    }
    let mut items = Vec::new();
    for (id, candidate) in candidates.into_iter().take(limit as usize) {
        let value = snapshot::read(
            store,
            &ResourceTarget::Sandbox {
                sandbox_id: SandboxId(id),
            },
            session,
        )
        .await?;
        let sandbox = match &scope {
            Scope::Organization(organization) => value.organization_sandbox(organization)?,
            Scope::Shared {
                recipient,
                organizations,
            } => value.shared_sandbox(recipient, organizations)?,
        };
        if sandbox != &candidate {
            return Err(ResourceAccessStoreError::Corrupt.into());
        }
        items.push(value);
    }
    Ok(items)
}
