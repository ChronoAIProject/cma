//! New Agent effects read publisher erasure fences after durable replay.
use super::{documents, majority_collection, TRIGGER_DEFINITIONS, TRIGGER_DEFINITION_REVISIONS};
use layer2_core::trigger_domain::{Launch, TriggerRevision};
use mongodb::{
    bson::{doc, Document},
    ClientSession, Database,
};

/// Existing admissions replay before reaching this fence. A retained anonymous
/// Profile can support an existing Agent, but cannot authorize a new effect for
/// a publisher whose definition or exact revision has been erased.
pub(in crate::store) async fn fence_new_agent_publisher(
    db: &Database,
    launch: &Launch,
    session: &mut ClientSession,
) -> mongodb::error::Result<bool> {
    Ok(surviving_launch_revision(db, launch, session)
        .await?
        .is_some())
}

pub(in crate::store) async fn surviving_launch_revision(
    db: &Database,
    launch: &Launch,
    session: &mut ClientSession,
) -> mongodb::error::Result<Option<TriggerRevision>> {
    let definitions = majority_collection::<Document>(db, TRIGGER_DEFINITIONS);
    let Some(definition) = definitions
        .find_one(doc! {
            "_id":launch.trigger_id.as_str(),
            "account_erasure_fenced":{"$ne":true},
        })
        .session(&mut *session)
        .await?
    else {
        return Ok(None);
    };
    let Ok(stored) = documents::read_trigger(&definition) else {
        return Ok(None);
    };
    if stored.trigger.id != launch.trigger_id
        || stored.trigger.public_id.as_ref() != Some(&launch.public_id)
    {
        return Ok(None);
    }
    let Some(expected) = stored
        .trigger
        .revisions
        .iter()
        .rev()
        .find(|revision| revision.digest == launch.revision_digest)
    else {
        return Ok(None);
    };
    let revisions = majority_collection::<Document>(db, TRIGGER_DEFINITION_REVISIONS);
    let Some(revision) = revisions
        .find_one(doc! {
            "_id":documents::revision_key(launch.trigger_id.as_str(), expected.revision),
            "account_erasure_fenced":{"$ne":true},
        })
        .session(&mut *session)
        .await?
    else {
        return Ok(None);
    };
    let Ok(published) = documents::read_revision(&revision) else {
        return Ok(None);
    };
    if published.revision.publisher_deleted
        || published.trigger_id != launch.trigger_id.as_str()
        || published.public_id != launch.public_id
        || published.owner != stored.trigger.owner.as_str()
        || &published.revision != expected
    {
        return Ok(None);
    }
    // Erasure commits these fences and waits for older transactions before
    // discovering references. Ordinary pause/retirement still permits accepted
    // Launch continuation, without creating catalogue write contention.
    Ok(Some(published.revision))
}
