use std::collections::{BTreeSet, HashMap};

use futures_util::TryStreamExt;
use layer2_core::ids::SubjectId;
use layer2_core::trigger_domain::{
    PublicTrigger, PublicTriggerId, Trigger, TriggerAvailability, TriggerId, TriggerPage,
    TriggerPageRequest, TriggerRevision, TriggerStoreError,
};
use mongodb::bson::{doc, Bson, Document};
use mongodb::ClientSession;

use super::documents::{read_revision, read_trigger, StoredRevision, StoredTrigger};
use super::{driver_error, MongoTriggerDefinitionStore};

const EXACT_REVISION_LIMIT: i64 = 1;

pub(super) async fn owned(
    store: &MongoTriggerDefinitionStore,
    id: &TriggerId,
    owner: &SubjectId,
) -> Result<Option<StoredTrigger>, TriggerStoreError> {
    let document = store
        .definitions
        .find_one(doc! { "_id": id.as_str(), "owner": owner.as_str(), "archived_at": Bson::Null })
        .await
        .map_err(|error| driver_error("read owned Trigger definition", error))?;
    let Some(document) = document else {
        return Ok(None);
    };
    let stored = read_trigger(&document)?;
    verify_revisions(store, std::slice::from_ref(&stored.trigger)).await?;
    Ok(Some(stored))
}

pub(super) async fn by_id(
    store: &MongoTriggerDefinitionStore,
    id: &TriggerId,
) -> Result<Option<StoredTrigger>, TriggerStoreError> {
    let document = store
        .definitions
        .find_one(doc! { "_id": id.as_str() })
        .await
        .map_err(|error| driver_error("read Trigger definition", error))?;
    let Some(document) = document else {
        return Ok(None);
    };
    let stored = read_trigger(&document)?;
    verify_revisions(store, std::slice::from_ref(&stored.trigger)).await?;
    Ok(Some(stored))
}

pub(super) async fn public(
    store: &MongoTriggerDefinitionStore,
    id: &PublicTriggerId,
) -> Result<Option<PublicTrigger>, TriggerStoreError> {
    let document = store
        .definitions
        .find_one(doc! { "public_id": id.as_str() })
        .await
        .map_err(|error| driver_error("read public Trigger definition", error))?;
    let Some(document) = document else {
        return Ok(None);
    };
    let stored = read_trigger(&document)?;
    verify_revisions(store, std::slice::from_ref(&stored.trigger)).await?;
    project_public(stored.trigger).map(Some)
}

pub(super) async fn revision(
    store: &MongoTriggerDefinitionStore,
    id: &TriggerId,
    digest: &str,
) -> Result<Option<TriggerRevision>, TriggerStoreError> {
    let mut cursor = store
        .revisions
        .find(doc! { "trigger_id": id.as_str(), "record.digest": digest })
        .sort(doc! { "revision": 1_i32 })
        .limit(EXACT_REVISION_LIMIT)
        .await
        .map_err(|error| driver_error("read exact immutable Trigger revision", error))?;
    let Some(document) = cursor
        .try_next()
        .await
        .map_err(|error| driver_error("stream exact immutable Trigger revision", error))?
    else {
        return Ok(None);
    };
    let stored = read_revision(&document)?;
    if stored.trigger_id != id.as_str() || stored.revision.digest != digest {
        return Err(TriggerStoreError::Corrupt(
            "immutable Trigger revision crossed its lookup boundary".into(),
        ));
    }
    stored
        .revision
        .validate()
        .map_err(|error| TriggerStoreError::Corrupt(error.to_string()))?;
    Ok(Some(stored.revision))
}

pub(super) async fn list_owner(
    store: &MongoTriggerDefinitionStore,
    owner: &SubjectId,
    page: &TriggerPageRequest,
) -> Result<TriggerPage<Trigger>, TriggerStoreError> {
    page.validate()?;
    let mut filter = doc! { "owner": owner.as_str(), "archived_at": Bson::Null };
    if let Some(cursor) = &page.cursor {
        filter.insert("_id", doc! { "$gt": cursor });
    }
    let mut cursor = store
        .definitions
        .find(filter)
        .sort(doc! { "_id": 1_i32 })
        .limit(i64::from(page.limit) + 1)
        .await
        .map_err(|error| driver_error("list owned Trigger definitions", error))?;
    let mut stored = Vec::new();
    while let Some(document) = cursor
        .try_next()
        .await
        .map_err(|error| driver_error("read owned Trigger definition page", error))?
    {
        let row = read_trigger(&document)?;
        if &row.trigger.owner != owner {
            return Err(TriggerStoreError::Corrupt(
                "Trigger owner index crossed its owner boundary".into(),
            ));
        }
        stored.push(row.trigger);
    }
    let has_more = stored.len() > usize::from(page.limit);
    if has_more {
        stored.pop();
    }
    verify_revisions(store, &stored).await?;
    let next = has_more.then(|| {
        stored
            .last()
            .expect("a nonempty bounded Trigger page has a last row")
            .id
            .to_string()
    });
    Ok(TriggerPage {
        items: stored,
        next,
    })
}

pub(super) async fn verify_in_session(
    store: &MongoTriggerDefinitionStore,
    trigger: &Trigger,
    session: &mut ClientSession,
) -> Result<(), TransactionReadError> {
    let mut cursor = store
        .revisions
        .find(doc! { "trigger_id": trigger.id.as_str() })
        .sort(doc! { "revision": 1_i32 })
        .session(&mut *session)
        .await
        .map_err(TransactionReadError::Driver)?;
    let documents = cursor
        .stream(&mut *session)
        .try_collect::<Vec<_>>()
        .await
        .map_err(TransactionReadError::Driver)?;
    let rows = documents
        .iter()
        .map(read_revision)
        .collect::<Result<Vec<_>, _>>()
        .map_err(TransactionReadError::Domain)?;
    compare_revisions(trigger, &rows).map_err(TransactionReadError::Domain)
}

pub(super) enum TransactionReadError {
    Domain(TriggerStoreError),
    Driver(mongodb::error::Error),
}

pub(super) async fn guard(
    store: &MongoTriggerDefinitionStore,
    id: &str,
    latest_field: &str,
    revision_field: &str,
    embedded_path: &str,
    referenced: impl Fn(&TriggerRevision) -> bool,
) -> Result<(), TriggerStoreError> {
    let mut candidates = BTreeSet::new();
    let mut revision_filter = Document::new();
    revision_filter.insert(revision_field, id);
    let mut revisions = store
        .revisions
        .find(revision_filter)
        .await
        .map_err(|error| driver_error("scan Trigger profile references", error))?;
    while let Some(document) = revisions
        .try_next()
        .await
        .map_err(|error| driver_error("read Trigger profile reference", error))?
    {
        candidates.insert(read_revision(&document)?.trigger_id);
    }

    let mut latest = Document::new();
    latest.insert(latest_field, id);
    let mut embedded = Document::new();
    embedded.insert(embedded_path, id);
    let mut definitions = store
        .definitions
        .find(doc! { "$or": [latest, embedded] })
        .await
        .map_err(|error| driver_error("scan Trigger aggregate references", error))?;
    while let Some(document) = definitions
        .try_next()
        .await
        .map_err(|error| driver_error("read Trigger aggregate reference", error))?
    {
        candidates.insert(read_trigger(&document)?.trigger.id.to_string());
    }

    let mut blocking = Vec::new();
    for candidate in candidates {
        let trigger_id = TriggerId(candidate.clone());
        let Some(stored) = by_id(store, &trigger_id).await? else {
            return Err(TriggerStoreError::Corrupt(format!(
                "Trigger revision {candidate} has no aggregate"
            )));
        };
        if stored.trigger.availability == TriggerAvailability::Published
            && stored.trigger.latest_revision().is_some_and(&referenced)
        {
            blocking.push(stored.trigger.id);
        }
    }
    blocking.sort();
    if blocking.is_empty() {
        Ok(())
    } else {
        Err(TriggerStoreError::ProfileReferenced {
            trigger_ids: blocking,
        })
    }
}

pub(super) async fn verify_revisions(
    store: &MongoTriggerDefinitionStore,
    triggers: &[Trigger],
) -> Result<(), TriggerStoreError> {
    if triggers.is_empty() {
        return Ok(());
    }
    let ids = triggers
        .iter()
        .map(|trigger| Bson::String(trigger.id.to_string()))
        .collect::<Vec<_>>();
    let mut cursor = store
        .revisions
        .find(doc! { "trigger_id": { "$in": ids } })
        .sort(doc! { "trigger_id": 1_i32, "revision": 1_i32 })
        .await
        .map_err(|error| driver_error("read immutable Trigger revisions", error))?;
    let mut rows: HashMap<String, Vec<StoredRevision>> = HashMap::new();
    while let Some(document) = cursor
        .try_next()
        .await
        .map_err(|error| driver_error("stream immutable Trigger revisions", error))?
    {
        let row = read_revision(&document)?;
        rows.entry(row.trigger_id.clone()).or_default().push(row);
    }
    for trigger in triggers {
        compare_revisions(
            trigger,
            rows.remove(trigger.id.as_str())
                .as_deref()
                .unwrap_or_default(),
        )?;
    }
    if !rows.is_empty() {
        return Err(TriggerStoreError::Corrupt(
            "immutable Trigger revision query crossed its ID boundary".into(),
        ));
    }
    Ok(())
}

fn compare_revisions(trigger: &Trigger, rows: &[StoredRevision]) -> Result<(), TriggerStoreError> {
    if rows.len() != trigger.revisions.len() {
        return Err(TriggerStoreError::Corrupt(format!(
            "Trigger {} revision inventory disagrees with its aggregate",
            trigger.id
        )));
    }
    for (expected, stored) in trigger.revisions.iter().zip(rows) {
        if stored.trigger_id != trigger.id.as_str()
            || stored.owner != trigger.owner.as_str()
            || Some(&stored.public_id) != trigger.public_id.as_ref()
            || &stored.revision != expected
        {
            return Err(TriggerStoreError::Corrupt(format!(
                "Trigger {} immutable revision disagrees with its aggregate",
                trigger.id
            )));
        }
    }
    Ok(())
}

fn project_public(trigger: Trigger) -> Result<PublicTrigger, TriggerStoreError> {
    let revision = trigger.latest_revision().cloned().ok_or_else(|| {
        TriggerStoreError::Corrupt("public Trigger has no immutable revision".into())
    })?;
    let public_id = trigger
        .public_id
        .ok_or_else(|| TriggerStoreError::Corrupt("public Trigger has no public ID".into()))?;
    Ok(PublicTrigger {
        trigger_id: trigger.id,
        public_id,
        owner: trigger.owner,
        availability: trigger.availability,
        revision,
    })
}
