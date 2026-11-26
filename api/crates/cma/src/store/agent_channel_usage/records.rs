use super::*;
use crate::store::sandbox_mongo::{read_record, record_bson, DOCUMENT_SCHEMA_VERSION};
use layer2_core::sandbox::store::{channel_usage_cursor, channel_usage_page};
use mongodb::{
    options::{IndexOptions, ReadConcern},
    Collection, IndexModel,
};

fn corrupt() -> SandboxStoreError {
    SandboxStoreError::Corrupt("channel usage document or index is inconsistent".into())
}

pub(super) fn event_document(event: &ChannelUsageEvent) -> Result<Document, SandboxStoreError> {
    event.validate()?;
    let mut document = doc! {
        "_id":&event.operation_id, "document_schema_version":DOCUMENT_SCHEMA_VERSION,
        "agent_id":event.after.creation.agent_id.as_str(), "revision":event.after.revision as i64,
        "digest":event.digest(), "record":record_bson(event).map_err(|_| corrupt())?,
    };
    if let Some(binding) = event.change.introduced_binding() {
        document.insert("binding_id", &binding.binding_id);
    }
    Ok(document)
}

pub(in crate::store) fn read_event(
    document: &Document,
) -> Result<ChannelUsageEvent, SandboxStoreError> {
    let event: ChannelUsageEvent = read_record(document).map_err(|_| corrupt())?;
    let expected = event_document(&event)?;
    if expected
        .iter()
        .any(|(key, value)| key != "record" && document.get(key) != Some(value))
        || document.contains_key("binding_id") != expected.contains_key("binding_id")
    {
        return Err(corrupt());
    }
    Ok(event)
}

pub(super) fn state_document(
    stored: &StoredAgentChannelUsage,
) -> Result<Document, SandboxStoreError> {
    stored.state.validate()?;
    if stored.state.revision == 0 {
        return Err(corrupt());
    }
    Ok(doc! {
        "_id":stored.state.creation.agent_id.as_str(), "document_schema_version":DOCUMENT_SCHEMA_VERSION,
        "revision":stored.state.revision as i64, "active":stored.state.summary().active,
        "record":record_bson(stored).map_err(|_| corrupt())?,
    })
}

pub(in crate::store) fn read_state(
    document: &Document,
) -> Result<StoredAgentChannelUsage, SandboxStoreError> {
    let stored = read_record(document).map_err(|_| corrupt())?;
    let expected = state_document(&stored)?;
    if expected
        .iter()
        .any(|(key, value)| key != "record" && document.get(key) != Some(value))
    {
        return Err(corrupt());
    }
    Ok(stored)
}

pub(in crate::store) async fn read_snapshot(
    store: &MongoSandboxStore,
    id: &AgentId,
    session: &mut ClientSession,
) -> Result<AgentChannelUsageSnapshot, UsageError> {
    let provenance = crate::store::agent_provenance::read_snapshot(store, id, session).await?;
    let state = match read_retained_state(store, id, session, false).await? {
        Some(stored) => stored.state,
        None => AgentChannelUsageState::empty(provenance.creation_provenance(id)?),
    };
    let snapshot = AgentChannelUsageSnapshot {
        provenance,
        state,
        task_status: Default::default(),
    };
    snapshot.validate(id)?;
    Ok(snapshot)
}

/// History belongs to its original Agent incarnation, even after removal.
/// Reading it supplies no current resource access or dispatch authority.
pub(super) async fn read_retained_state(
    store: &MongoSandboxStore,
    id: &AgentId,
    session: &mut ClientSession,
    snapshot: bool,
) -> Result<Option<StoredAgentChannelUsage>, UsageError> {
    // Snapshot sessions add atClusterTime without overriding an inherited
    // majority level in the driver. Each read must select snapshot explicitly;
    // transaction callers leave their concern to start_transaction instead.
    let query = store.agent_channel_usage.find_one(doc! {"_id":id.as_str()});
    let query = if snapshot {
        query.read_concern(ReadConcern::snapshot())
    } else {
        query
    };
    let stored = query
        .session(&mut *session)
        .await?
        .as_ref()
        .map(read_state)
        .transpose()?;
    let query = store
        .channel_usage_events
        .find_one(doc! {"agent_id":id.as_str()})
        .sort(doc! {"revision":-1});
    let query = if snapshot {
        query.read_concern(ReadConcern::snapshot())
    } else {
        query
    };
    let latest = query
        .session(session)
        .await?
        .as_ref()
        .map(read_event)
        .transpose()?;
    match (stored, latest) {
        (Some(stored), Some(latest)) => {
            stored.validate_latest(&latest)?;
            if &stored.state.creation.agent_id != id {
                return Err(corrupt().into());
            }
            Ok(Some(stored))
        }
        (None, None) => Ok(None),
        _ => Err(corrupt().into()),
    }
}

pub(super) async fn read_history(
    store: &MongoSandboxStore,
    id: &AgentId,
    page: &PageRequest,
    session: &mut ClientSession,
) -> Result<(AgentChannelUsageSnapshot, Page<ChannelUsageEvent>), UsageError> {
    let cursor = channel_usage_cursor(page)?;
    let snapshot = read_snapshot(store, id, session).await?;
    let preceding = if cursor == 0 {
        None
    } else {
        store
            .channel_usage_events
            .find_one(doc! {"agent_id":id.as_str(),"revision":cursor as i64})
            .session(&mut *session)
            .await?
            .as_ref()
            .map(read_event)
            .transpose()?
    };
    let mut rows = store
        .channel_usage_events
        .find(doc! {"agent_id":id.as_str(),"revision":{"$gt":cursor as i64}})
        .sort(doc! {"revision":1})
        .limit(i64::from(page.limit) + 1)
        .session(&mut *session)
        .await?;
    let mut events = Vec::new();
    while rows.advance(&mut *session).await? {
        events.push(read_event(&rows.deserialize_current()?)?);
    }
    let page = channel_usage_page(&snapshot, page, preceding.as_ref(), events)?;
    Ok((snapshot, page))
}

pub(in crate::store) async fn ensure_indexes(
    collection: &Collection<Document>,
) -> Result<(), layer2_core::StoreError> {
    let indexes = [
        // Historical-only DrainOriginal use has no synthetic Attach event.
        // This index resolves its exact reference for a later explicit reset.
        IndexModel::builder()
            .keys(doc! {"record.predecessor.binding_id":1})
            .build(),
        IndexModel::builder()
            .keys(doc! {"agent_id":1,"revision":1})
            .options(IndexOptions::builder().unique(true).build())
            .build(),
        IndexModel::builder()
            .keys(doc! {"binding_id":1})
            .options(
                IndexOptions::builder()
                    .unique(true)
                    .partial_filter_expression(doc! {"binding_id":{"$type":"string"}})
                    .build(),
            )
            .build(),
    ];
    collection
        .create_indexes(indexes)
        .await
        .map(|_| ())
        .map_err(|_| {
            layer2_core::StoreError::Unavailable("create channel usage history indexes".into())
        })
}
