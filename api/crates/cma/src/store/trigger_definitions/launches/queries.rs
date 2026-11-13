use std::collections::BTreeMap;

use layer2_core::ids::{
    LaunchId, PlacementId, PublicTriggerId, ServiceHandoffId, SubjectId, TriggerId,
};
use layer2_core::trigger_domain::{
    Launch, LaunchPage, LaunchPageRequest, LaunchPlacementStats, LaunchStoreError,
    ServiceConnectionHandoff,
};
use mongodb::bson::{doc, Document};
use mongodb::options::ReadConcern;
use mongodb::ClientSession;

use super::documents::{read_handoff, read_launch, StoredLaunch};
use super::{corrupt, unavailable, MongoTriggerDefinitionStore};
use super::{event_log, input_protocol};

mod snapshots;
pub(super) use snapshots::{snapshot_handoff, snapshot_launch, snapshot_owned_launch};

pub(super) async fn by_command(
    store: &MongoTriggerDefinitionStore,
    owner: &SubjectId,
    public_id: &PublicTriggerId,
    command_key_hash: &str,
) -> Result<Option<Launch>, LaunchStoreError> {
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .await
        .map_err(unavailable)?;
    let document = store
        .launch_data
        .launches
        .find_one(command_filter(owner, public_id, command_key_hash))
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .await
        .map_err(unavailable)?;
    let mut launch = decode_command(document.as_ref(), owner, public_id, command_key_hash)?;
    if let Some(launch) = &mut launch {
        input_protocol::verify_snapshot(store, launch, &mut session).await?;
        event_log::verify_snapshot(store, launch, &mut session).await?;
    }
    Ok(launch)
}

pub(super) async fn owned(
    store: &MongoTriggerDefinitionStore,
    id: &LaunchId,
    owner: &SubjectId,
) -> Result<Option<Launch>, LaunchStoreError> {
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .await
        .map_err(unavailable)?;
    snapshot_owned_launch(store, id, owner, &mut session).await
}

pub(super) async fn list_owner(
    store: &MongoTriggerDefinitionStore,
    owner: &SubjectId,
    page: &LaunchPageRequest,
) -> Result<LaunchPage, LaunchStoreError> {
    page.validate()?;
    let boundary = page.cursor_boundary()?;
    let mut clauses = vec![doc! { "owner": owner.as_str() }];
    if let Some(trigger) = page.trigger.as_ref() {
        clauses.push(doc! { "trigger_id": trigger.as_str() });
    }
    if let Some(state) = page.state {
        let state =
            mongodb::bson::serialize_to_bson(&state).map_err(|error| corrupt(error.to_string()))?;
        clauses.push(doc! { "state": state });
    }
    if let Some(boundary) = boundary.as_ref() {
        clauses.push(doc! {
            "$or": [
                { "created_at": { "$lt": &boundary.created_at } },
                {
                    "created_at": &boundary.created_at,
                    "_id": { "$lt": boundary.id.as_str() },
                },
            ],
        });
    }
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .await
        .map_err(unavailable)?;
    let mut cursor = store
        .launch_data
        .launches
        .find(doc! { "$and": clauses })
        .sort(doc! { "created_at": -1_i32, "_id": -1_i32 })
        .limit(i64::from(page.limit.saturating_add(1)))
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .await
        .map_err(unavailable)?;
    let mut launches = Vec::new();
    while let Some(document) = cursor.next(&mut session).await {
        let document = document.map_err(unavailable)?;
        let mut launch = read_launch(&document)?.launch;
        input_protocol::verify_snapshot(store, &mut launch, &mut session).await?;
        validate_listed(&launch, owner, page, boundary.as_ref(), launches.last())?;
        launches.push(launch);
    }
    let has_more = launches.len() > usize::from(page.limit);
    launches.truncate(usize::from(page.limit));
    let next = if has_more {
        launches
            .last()
            .map(|launch| page.cursor_after(launch))
            .transpose()?
    } else {
        None
    };
    Ok(LaunchPage {
        items: launches,
        next,
    })
}

pub(super) async fn source_launch(
    store: &MongoTriggerDefinitionStore,
    owner: &SubjectId,
    agent_id: &layer2_core::AgentId,
    trigger: Option<&TriggerId>,
) -> Result<Option<Launch>, LaunchStoreError> {
    let mut query = doc! {
        "owner": owner.as_str(),
        "record.resolution.agent.id": agent_id.as_str(),
    };
    if let Some(trigger) = trigger {
        query.insert("trigger_id", trigger.as_str());
    }
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .await
        .map_err(unavailable)?;
    let document = store
        .launch_data
        .launches
        .find_one(query)
        .sort(doc! { "created_at": 1, "_id": 1 })
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .await
        .map_err(unavailable)?;
    let Some(document) = document else {
        return Ok(None);
    };
    let mut launch = read_launch(&document)?.launch;
    input_protocol::verify_snapshot(store, &mut launch, &mut session).await?;
    if launch.owner != *owner
        || launch.resolution.agent.id() != Some(agent_id)
        || trigger.is_some_and(|id| *id != launch.trigger_id)
    {
        return Err(corrupt(
            "Agent source query crossed its owned Agent boundary",
        ));
    }
    Ok(Some(launch))
}

pub(super) async fn placement_stats(
    store: &MongoTriggerDefinitionStore,
    trigger_id: &TriggerId,
) -> Result<Vec<LaunchPlacementStats>, LaunchStoreError> {
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .await
        .map_err(unavailable)?;
    let mut cursor = store
        .launch_data
        .launches
        .find(doc! { "trigger_id": trigger_id.as_str() })
        .sort(doc! { "_id": 1_i32 })
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .await
        .map_err(unavailable)?;
    let mut stats = BTreeMap::<Option<PlacementId>, (u64, u64)>::new();
    while let Some(document) = cursor.next(&mut session).await {
        let document = document.map_err(unavailable)?;
        let mut launch = read_launch(&document)?.launch;
        input_protocol::verify_snapshot(store, &mut launch, &mut session).await?;
        if &launch.trigger_id != trigger_id {
            return Err(corrupt("placement stats crossed their Trigger boundary"));
        }
        let entry = stats.entry(launch.placement_id).or_default();
        entry.0 = entry
            .0
            .checked_add(1)
            .ok_or_else(|| corrupt("placement Launch count overflow"))?;
        if launch.ready_at.is_some() {
            entry.1 = entry
                .1
                .checked_add(1)
                .ok_or_else(|| corrupt("placement ready-Agent count overflow"))?;
        }
    }
    Ok(stats
        .into_iter()
        .map(
            |(placement_id, (launches_total, agents_ready_total))| LaunchPlacementStats {
                placement_id,
                launches_total,
                agents_ready_total,
            },
        )
        .collect())
}

fn validate_listed(
    launch: &Launch,
    owner: &SubjectId,
    page: &LaunchPageRequest,
    boundary: Option<&layer2_core::trigger_domain::LaunchPageBoundary>,
    prior: Option<&Launch>,
) -> Result<(), LaunchStoreError> {
    if &launch.owner != owner
        || page
            .trigger
            .as_ref()
            .is_some_and(|trigger| trigger != &launch.trigger_id)
        || page.state.is_some_and(|state| state != launch.state)
        || boundary.is_some_and(|boundary| {
            (&launch.created_at, &launch.id) >= (&boundary.created_at, &boundary.id)
        })
        || prior
            .is_some_and(|prior| (&prior.created_at, &prior.id) <= (&launch.created_at, &launch.id))
    {
        return Err(corrupt("Launch list query crossed its requested boundary"));
    }
    Ok(())
}

pub(super) async fn by_id(
    store: &MongoTriggerDefinitionStore,
    id: &LaunchId,
) -> Result<Option<StoredLaunch>, LaunchStoreError> {
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .await
        .map_err(unavailable)?;
    snapshot_launch(store, id, &mut session).await
}

pub(super) async fn handoff_by_intent(
    store: &MongoTriggerDefinitionStore,
    owner: &SubjectId,
    launch_id: &LaunchId,
    intent_id: &str,
) -> Result<Option<ServiceConnectionHandoff>, LaunchStoreError> {
    let document = store
        .launch_data
        .handoffs
        .find_one(doc! {
            "owner": owner.as_str(),
            "launch_id": launch_id.as_str(),
            "intent_id": intent_id,
        })
        .await
        .map_err(unavailable)?;
    decode_handoff_command(document.as_ref(), owner, launch_id, intent_id)
}

pub(super) async fn due(
    store: &MongoTriggerDefinitionStore,
    now: &str,
    limit: u16,
) -> Result<Vec<LaunchId>, LaunchStoreError> {
    if limit == 0 || limit > layer2_core::trigger_domain::MAX_LAUNCH_EVENT_LIMIT {
        return Err(LaunchStoreError::InvalidCursor);
    }
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .await
        .map_err(unavailable)?;
    let mut cursor = store
        .launch_data
        .launches
        .find(super::due::filter(now))
        .sort(doc! { "work_ready_at": 1_i32, "_id": 1_i32 })
        .limit(i64::from(limit))
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .await
        .map_err(unavailable)?;
    let mut launches = Vec::new();
    while let Some(document) = cursor.next(&mut session).await {
        let document = document.map_err(unavailable)?;
        let mut launch = read_launch(&document)?.launch;
        input_protocol::verify_snapshot(store, &mut launch, &mut session).await?;
        if !launch.is_worker_due(now) {
            return Err(corrupt("due-work query returned a Launch that is not due"));
        }
        if launches.last().is_some_and(|prior: &Launch| {
            (&prior.work.ready_at, &prior.id) >= (&launch.work.ready_at, &launch.id)
        }) {
            return Err(corrupt("due-work query returned Launches out of order"));
        }
        launches.push(launch);
    }
    Ok(launches.into_iter().map(|launch| launch.id).collect())
}

pub(super) async fn launch_in_session(
    store: &MongoTriggerDefinitionStore,
    id: &LaunchId,
    session: &mut ClientSession,
) -> Result<Option<StoredLaunch>, ReadError> {
    let mut result = store
        .launch_data
        .launches
        .find_one(doc! { "_id": id.as_str() })
        .session(&mut *session)
        .await
        .map_err(ReadError::Driver)?
        .as_ref()
        .map(read_launch)
        .transpose()
        .map_err(ReadError::Domain)?;
    if let Some(stored) = &mut result {
        input_protocol::verify_in_session(store, &mut stored.launch, session).await?;
    }
    Ok(result)
}

pub(super) async fn command_in_session(
    store: &MongoTriggerDefinitionStore,
    owner: &SubjectId,
    public_id: &PublicTriggerId,
    command_key_hash: &str,
    session: &mut ClientSession,
) -> Result<Option<Launch>, ReadError> {
    let document = store
        .launch_data
        .launches
        .find_one(command_filter(owner, public_id, command_key_hash))
        .session(&mut *session)
        .await
        .map_err(ReadError::Driver)?;
    let mut launch = decode_command(document.as_ref(), owner, public_id, command_key_hash)
        .map_err(ReadError::Domain)?;
    if let Some(launch) = &mut launch {
        input_protocol::verify_in_session(store, launch, session).await?;
        event_log::verify_in_session(store, launch, session).await?;
    }
    Ok(launch)
}

pub(super) async fn handoff_in_session(
    store: &MongoTriggerDefinitionStore,
    id: &ServiceHandoffId,
    session: &mut ClientSession,
) -> Result<Option<ServiceConnectionHandoff>, ReadError> {
    store
        .launch_data
        .handoffs
        .find_one(doc! { "_id": id.as_str() })
        .session(session)
        .await
        .map_err(ReadError::Driver)?
        .as_ref()
        .map(read_handoff)
        .transpose()
        .map_err(ReadError::Domain)
}

pub(super) async fn handoff_command_in_session(
    store: &MongoTriggerDefinitionStore,
    owner: &SubjectId,
    launch_id: &LaunchId,
    intent_id: &str,
    session: &mut ClientSession,
) -> Result<Option<ServiceConnectionHandoff>, ReadError> {
    let document = store
        .launch_data
        .handoffs
        .find_one(doc! {
            "owner": owner.as_str(),
            "launch_id": launch_id.as_str(),
            "intent_id": intent_id,
        })
        .session(session)
        .await
        .map_err(ReadError::Driver)?;
    decode_handoff_command(document.as_ref(), owner, launch_id, intent_id)
        .map_err(ReadError::Domain)
}

fn decode_command(
    document: Option<&Document>,
    owner: &SubjectId,
    public_id: &PublicTriggerId,
    command_key_hash: &str,
) -> Result<Option<Launch>, LaunchStoreError> {
    let Some(document) = document else {
        return Ok(None);
    };
    let launch = read_launch(document)?.launch;
    if &launch.owner != owner
        || &launch.public_id != public_id
        || launch.command_key_hash != command_key_hash
    {
        return Err(corrupt("Launch command points to a different aggregate"));
    }
    Ok(Some(launch))
}

fn decode_handoff_command(
    document: Option<&Document>,
    owner: &SubjectId,
    launch_id: &LaunchId,
    intent_id: &str,
) -> Result<Option<ServiceConnectionHandoff>, LaunchStoreError> {
    let Some(document) = document else {
        return Ok(None);
    };
    let handoff = read_handoff(document)?;
    if &handoff.owner != owner || &handoff.launch_id != launch_id || handoff.intent_id != intent_id
    {
        return Err(corrupt(
            "service handoff command points to a different record",
        ));
    }
    Ok(Some(handoff))
}

fn command_filter(
    owner: &SubjectId,
    public_id: &PublicTriggerId,
    command_key_hash: &str,
) -> Document {
    doc! {
        "owner": owner.as_str(),
        "public_id": public_id.as_str(),
        "command_key_hash": command_key_hash,
    }
}

pub(super) enum ReadError {
    Domain(LaunchStoreError),
    Driver(mongodb::error::Error),
}
