use futures_util::TryStreamExt;
use layer2_core::ids::{LaunchId, SubjectId};
use layer2_core::trigger_domain::{Launch, LaunchEvent, LaunchStoreError, MAX_LAUNCH_EVENT_LIMIT};
use mongodb::bson::{doc, Document};
use mongodb::options::ReadConcern;
use mongodb::ClientSession;

use crate::store::sandbox_mongo::signed;

use super::documents::read_event;
use super::queries::{self, ReadError};
use super::{corrupt, unavailable, MongoTriggerDefinitionStore};

pub(super) async fn events(
    store: &MongoTriggerDefinitionStore,
    id: &LaunchId,
    owner: &SubjectId,
    after: u64,
    limit: u16,
) -> Result<Vec<LaunchEvent>, LaunchStoreError> {
    validate_limit(limit)?;
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .await
        .map_err(unavailable)?;
    let launch = queries::snapshot_owned_launch(store, id, owner, &mut session)
        .await?
        .ok_or(LaunchStoreError::NotFound)?;
    if after > launch.event_head {
        return Err(LaunchStoreError::InvalidCursor);
    }
    let count = (launch.event_head - after).min(u64::from(limit));
    let mut cursor = store
        .launch_data
        .events
        .find(doc! {
            "launch_id": id.as_str(),
            "seq": { "$gt": storage_integer(after, "Launch event cursor")? },
        })
        .read_concern(ReadConcern::snapshot())
        .sort(doc! { "seq": 1_i32 })
        .limit(i64::from(limit))
        .session(&mut session)
        .await
        .map_err(unavailable)?;
    let mut rows = Vec::with_capacity(count as usize);
    while let Some(document) = cursor
        .stream(&mut session)
        .try_next()
        .await
        .map_err(unavailable)?
    {
        rows.push(read_event(&document)?);
    }
    validate_slice(&rows, id, after + 1, count as usize)?;
    Ok(rows)
}

pub(super) async fn verify_snapshot(
    store: &MongoTriggerDefinitionStore,
    launch: &Launch,
    session: &mut ClientSession,
) -> Result<(), LaunchStoreError> {
    let mut cursor = store
        .launch_data
        .events
        .find(doc! { "launch_id": launch.id.as_str() })
        .read_concern(ReadConcern::snapshot())
        .sort(doc! { "seq": 1_i32 })
        .session(&mut *session)
        .await
        .map_err(unavailable)?;
    let documents = cursor
        .stream(&mut *session)
        .try_collect::<Vec<Document>>()
        .await
        .map_err(unavailable)?;
    let rows = decode(&documents)?;
    validate_complete(&rows, launch)
}

pub(super) async fn verify_in_session(
    store: &MongoTriggerDefinitionStore,
    launch: &Launch,
    session: &mut ClientSession,
) -> Result<(), ReadError> {
    let mut cursor = store
        .launch_data
        .events
        .find(doc! { "launch_id": launch.id.as_str() })
        .sort(doc! { "seq": 1_i32 })
        .session(&mut *session)
        .await
        .map_err(ReadError::Driver)?;
    let documents = cursor
        .stream(&mut *session)
        .try_collect::<Vec<Document>>()
        .await
        .map_err(ReadError::Driver)?;
    let rows = decode(&documents).map_err(ReadError::Domain)?;
    validate_complete(&rows, launch).map_err(ReadError::Domain)
}

pub(super) async fn exists_in_session(
    store: &MongoTriggerDefinitionStore,
    id: &LaunchId,
    session: &mut ClientSession,
) -> Result<bool, ReadError> {
    store
        .launch_data
        .events
        .find_one(doc! { "launch_id": id.as_str() })
        .projection(doc! { "_id": 1_i32 })
        .session(session)
        .await
        .map(|row| row.is_some())
        .map_err(ReadError::Driver)
}

pub(super) async fn snapshot_matches(
    store: &MongoTriggerDefinitionStore,
    launch: &Launch,
    expected: &[LaunchEvent],
    session: &mut ClientSession,
) -> Result<bool, LaunchStoreError> {
    let mut cursor = store
        .launch_data
        .events
        .find(doc! { "launch_id": launch.id.as_str() })
        .read_concern(ReadConcern::snapshot())
        .sort(doc! { "seq": 1_i32 })
        .session(&mut *session)
        .await
        .map_err(unavailable)?;
    let documents = cursor
        .stream(&mut *session)
        .try_collect::<Vec<Document>>()
        .await
        .map_err(unavailable)?;
    let rows = decode(&documents)?;
    if validate_complete(&rows, launch).is_err() {
        return Ok(false);
    }
    for expected in expected {
        let Some(actual) = expected
            .seq
            .checked_sub(1)
            .and_then(|index| usize::try_from(index).ok())
            .and_then(|index| rows.get(index))
        else {
            return Ok(false);
        };
        if actual != expected {
            return Err(corrupt("Launch event sequence contains conflicting data"));
        }
    }
    Ok(true)
}

fn decode(documents: &[Document]) -> Result<Vec<LaunchEvent>, LaunchStoreError> {
    documents.iter().map(read_event).collect()
}

fn validate_complete(rows: &[LaunchEvent], launch: &Launch) -> Result<(), LaunchStoreError> {
    let count = usize::try_from(launch.event_head)
        .map_err(|_| corrupt("Launch event head exceeds this server's address space"))?;
    validate_slice(rows, &launch.id, 1, count)
}

fn validate_slice(
    rows: &[LaunchEvent],
    id: &LaunchId,
    first: u64,
    expected_count: usize,
) -> Result<(), LaunchStoreError> {
    if rows.len() != expected_count {
        return Err(corrupt("Launch event history has a gap or orphan row"));
    }
    for (offset, event) in rows.iter().enumerate() {
        let expected = first
            .checked_add(offset as u64)
            .ok_or_else(|| corrupt("Launch event sequence overflow"))?;
        if &event.launch_id != id || event.seq != expected {
            return Err(corrupt("Launch event history is not contiguous"));
        }
    }
    Ok(())
}

fn validate_limit(limit: u16) -> Result<(), LaunchStoreError> {
    if limit == 0 || limit > MAX_LAUNCH_EVENT_LIMIT {
        Err(LaunchStoreError::InvalidCursor)
    } else {
        Ok(())
    }
}

fn storage_integer(value: u64, field: &str) -> Result<i64, LaunchStoreError> {
    signed(value, field).map_err(corrupt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use layer2_core::trigger_domain::LaunchEventKind;

    fn event(seq: u64) -> LaunchEvent {
        LaunchEvent {
            launch_id: LaunchId("lch_one".into()),
            seq,
            appended_at: "2026-09-05T00:00:00Z".into(),
            kind: LaunchEventKind::Created,
        }
    }

    #[test]
    fn slices_reject_gaps_or_rows_above_the_head() {
        assert!(validate_slice(&[event(1), event(2)], &LaunchId("lch_one".into()), 1, 2).is_ok());
        assert!(validate_slice(&[event(1), event(3)], &LaunchId("lch_one".into()), 1, 2).is_err());
        assert!(validate_slice(&[event(1), event(2)], &LaunchId("lch_one".into()), 1, 1).is_err());
    }
}
