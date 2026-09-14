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
    PageRequest::new(query.page.limit, query.page.cursor.clone())
        .map_err(|_| ResourceAccessStoreError::Invalid)?;
    let mut filter = Document::new();
    if let Some(status) = query.status {
        filter.insert("status", status.name());
    }
    if let Some(cursor) = &query.page.cursor {
        filter.insert("_id", doc! {"$gt":cursor});
    }
    let limit = i64::from(query.page.limit) + 1;
    let branches = [
        (
            &store.resource_policies,
            doc! {"owner_kind":"organization","owner_subject":organization.as_str()},
            "_id",
        ),
        (
            &store.resource_shares,
            doc! {"target_org_id":organization.as_str(),"revoked":false,"selector_kind":"sandbox"},
            "selector_id",
        ),
    ];
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
        if value.organization_sandbox(organization)? != &candidate {
            return Err(ResourceAccessStoreError::Corrupt.into());
        }
        items.push(value);
    }
    Ok(items)
}
