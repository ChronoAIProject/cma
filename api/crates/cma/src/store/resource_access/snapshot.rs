use layer2_core::resource_access::{
    ResourceAccessSnapshot, ResourceAccessStoreError, ResourceTarget,
};
use mongodb::bson::doc;
use mongodb::ClientSession;

use super::super::sandbox_mongo::MongoSandboxStore;
use super::{records, snapshot_lineage, ReadError};

pub(super) async fn read(
    store: &MongoSandboxStore,
    target: &ResourceTarget,
    session: &mut ClientSession,
) -> Result<ResourceAccessSnapshot, ReadError> {
    let (lineage, policy) = snapshot_lineage::read(store, target, session).await?;
    let sandbox_id = lineage.sandbox.id.clone();
    let mut selectors = vec![doc! {"selector_kind":"sandbox","selector_id":sandbox_id.as_str()}];
    if let Some(agent) = &lineage.agent {
        selectors.push(doc! {"selector_kind":"agent","selector_id":agent.id.as_str()});
    }
    let mut cursor = store
        .resource_shares
        .find(doc! {"sandbox_id":sandbox_id.as_str(),"revoked":false,"$or":selectors})
        .sort(doc! {"_id": 1})
        .limit(layer2_core::resource_access::MAX_ACTIVE_RESOURCE_SHARES as i64 + 1)
        .session(&mut *session)
        .await?;
    let mut shares = Vec::new();
    while cursor.advance(&mut *session).await? {
        let document = cursor.deserialize_current()?;
        shares.push(records::read_share(&sandbox_id, &document)?);
        if shares.len() > layer2_core::resource_access::MAX_ACTIVE_RESOURCE_SHARES {
            return Err(ResourceAccessStoreError::Capacity.into());
        }
    }
    let snapshot = ResourceAccessSnapshot {
        lineage,
        policy,
        shares,
    };
    snapshot.scope(target)?;
    Ok(snapshot)
}
