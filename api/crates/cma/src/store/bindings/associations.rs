//! Owner-authorized membership reads over existing Manager records only.
use super::*;
use crate::store::transaction_terminal::GuardedTerminal as _;
use crate::store::{
    agent_provenance,
    resource_access::ReadError,
    sandbox_mongo::{is_transient, start_transaction, TRANSACTION_ATTEMPTS},
};
use layer2_core::{
    resource_access::ResourceAccessStoreError,
    sandbox::{provider_associations::*, store::PageRequest},
    AgentId, SubjectId,
};
use mongodb::{options::Hint, ClientSession};
use std::{collections::BTreeMap, time::Duration};

pub(super) const INDEX: &str = "owner_agent_association";
#[cfg(test)]
pub(super) fn index_keys() -> Document {
    doc! {"record.owner": 1, "record.agent_id": 1, "_id": 1}
}
fn query(owner: &SubjectId, id: &AgentId, after: Option<&str>) -> Document {
    let mut query = doc! {"record.owner": owner.as_str(), "record.agent_id": id.as_str()};
    if let Some(after) = after {
        query.insert("_id", doc! {"$gt": after});
    }
    query
}
#[derive(Debug)]
pub(super) enum Error {
    Driver(mongodb::error::Error),
    Domain(SandboxStoreError),
}
impl From<mongodb::error::Error> for Error {
    fn from(error: mongodb::error::Error) -> Self {
        Self::Driver(error)
    }
}
impl From<SandboxStoreError> for Error {
    fn from(error: SandboxStoreError) -> Self {
        Self::Domain(error)
    }
}
impl From<ReadError> for Error {
    fn from(error: ReadError) -> Self {
        match error {
            ReadError::Driver(error) => Self::Driver(error),
            ReadError::Domain(ResourceAccessStoreError::NotFound) => {
                Self::Domain(SandboxStoreError::NotFound)
            }
            ReadError::Domain(ResourceAccessStoreError::Conflict) => {
                Self::Domain(SandboxStoreError::Conflict)
            }
            ReadError::Domain(error) => Self::Domain(SandboxStoreError::Corrupt(error.to_string())),
        }
    }
}
fn unavailable() -> SandboxStoreError {
    SandboxStoreError::Unavailable("provider associations are unavailable".into())
}
#[async_trait::async_trait]
impl AgentProviderAssociationStore for MongoSandboxStore {
    async fn agent_provider_associations(
        &self,
        owner: &SubjectId,
        id: &AgentId,
        page: &PageRequest,
    ) -> Result<AgentProviderAssociationSnapshot, SandboxStoreError> {
        PageRequest::new(page.limit, None)?;
        for _ in 0..TRANSACTION_ATTEMPTS {
            let mut session = self
                .client
                .start_session()
                .await
                .map_err(|_| unavailable())?;
            start_transaction(&mut session)
                .await
                .map_err(|_| unavailable())?;
            let result = read(self, owner, id, page, &mut session).await;
            let _ = session.abort_guarded().await;
            match result {
                Ok(value) => return Ok(value),
                Err(Error::Driver(error)) if is_transient(&error) => continue,
                Err(Error::Domain(error)) => return Err(error),
                Err(Error::Driver(_)) => return Err(unavailable()),
            }
        }
        Err(unavailable())
    }
}
pub(super) async fn read(
    store: &MongoSandboxStore,
    owner: &SubjectId,
    id: &AgentId,
    page: &PageRequest,
    session: &mut ClientSession,
) -> Result<AgentProviderAssociationSnapshot, Error> {
    let provenance = agent_provenance::read_snapshot(store, id, session).await?;
    let creation = provenance.creation_provenance(id)?;
    if &provenance.resource.lineage.sandbox.owner != owner {
        return Err(SandboxStoreError::Conflict.into());
    }
    let after = association_cursor(page, &creation)?;
    let mut cursor = store
        .bindings
        .conversations
        .find(query(owner, id, after.as_deref()))
        .hint(Hint::Name(INDEX.into()))
        .sort(doc! {"_id": 1})
        .limit(i64::from(page.limit) + 1)
        .max_time(Duration::from_secs(10))
        .session(&mut *session)
        .await?;
    let mut conversations = Vec::with_capacity(usize::from(page.limit) + 1);
    while cursor.advance(&mut *session).await? {
        conversations.push(read_conversation(cursor.deserialize_current()?)?);
    }
    let mut parents = BTreeMap::new();
    if !conversations.is_empty() {
        let ids: Vec<String> = conversations.iter().map(|c| c.binding_id.clone()).collect();
        let mut cursor = store
            .bindings
            .records
            .find(doc! {"_id": {"$in": ids}})
            .limit(i64::from(page.limit) + 1)
            .max_time(Duration::from_secs(10))
            .session(&mut *session)
            .await?;
        while cursor.advance(&mut *session).await? {
            let binding = read_binding(cursor.deserialize_current()?)?;
            parents.insert(binding.binding_id.clone(), binding);
        }
    }
    let records = conversations
        .into_iter()
        .map(|conversation| {
            let binding = parents
                .get(&conversation.binding_id)
                .cloned()
                .ok_or_else(|| {
                    SandboxStoreError::Corrupt("provider association parent is missing".into())
                })?;
            Ok(ProviderAssociationRecord {
                conversation,
                binding,
            })
        })
        .collect::<Result<Vec<_>, SandboxStoreError>>()?;
    let snapshot = AgentProviderAssociationSnapshot {
        provenance,
        records,
    };
    association_page(&snapshot, owner, id, page)?;
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn query_is_owner_agent_scoped_before_exclusive_cursor_and_limit() {
        assert_eq!(
            index_keys(),
            doc! {"record.owner": 1, "record.agent_id": 1, "_id": 1}
        );
        assert_eq!(
            query(
                &SubjectId::new("owner"),
                &AgentId("agent".into()),
                Some("last")
            ),
            doc! {"record.owner":"owner", "record.agent_id":"agent", "_id":{"$gt":"last"}}
        );
        assert_eq!(
            query(&SubjectId::new("owner"), &AgentId("agent".into()), None).len(),
            2
        );
    }
}
