//! Read committed erasure fences while validating new catalogue pins.
use layer2_core::bots::BotProfiles;
use mongodb::{
    bson::{doc, Document},
    ClientSession, Database,
};

use super::durability::majority_collection;
use super::schema::DOCUMENT_SCHEMA_VERSION;
use super::{agent_profiles, sandbox_profiles, workspace_profiles};
pub(super) mod frozen;

pub(super) async fn bot_profiles(
    db: &Database,
    profiles: &BotProfiles,
    session: &mut ClientSession,
) -> mongodb::error::Result<bool> {
    Ok(sandbox(
        db,
        profiles.sandbox.profile_id.as_str(),
        profiles.sandbox.revision,
        Some(&profiles.sandbox.digest),
        session,
    )
    .await?
        && workspace(
            db,
            profiles.workspace.profile_id.as_str(),
            profiles.workspace.revision,
            Some(&profiles.workspace.digest),
            session,
        )
        .await?
        && agent(
            db,
            profiles.agent.profile_id.as_str(),
            profiles.agent.revision,
            Some(&profiles.agent.digest),
            session,
        )
        .await?)
}

pub(super) async fn sandbox(
    db: &Database,
    id: &str,
    revision: u32,
    digest: Option<&str>,
    session: &mut ClientSession,
) -> mongodb::error::Result<bool> {
    let Some(row) = exact(db, "sandbox_profile_revisions", id, revision, session).await? else {
        return Ok(false);
    };
    let Ok(pin) = sandbox_profiles::read_revision(&row) else {
        return Ok(false);
    };
    if digest.is_some_and(|expected| expected != pin.digest)
        || !unfenced(db, "sandbox_profiles", id, session).await?
    {
        return Ok(false);
    }
    // Profiles have a fixed three-level graph; content validation bounds each
    // child list. Every inherited exact pin shares the admission transaction.
    for reference in &pin.content.workspace_profiles {
        if !workspace(db, reference.id.as_str(), reference.revision, None, session).await? {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(super) async fn workspace(
    db: &Database,
    id: &str,
    revision: u32,
    digest: Option<&str>,
    session: &mut ClientSession,
) -> mongodb::error::Result<bool> {
    let Some(row) = exact(db, "workspace_profile_revisions", id, revision, session).await? else {
        return Ok(false);
    };
    let Ok(pin) = workspace_profiles::read_revision(&row) else {
        return Ok(false);
    };
    if digest.is_some_and(|expected| expected != pin.digest)
        || !unfenced(db, "workspace_profiles", id, session).await?
    {
        return Ok(false);
    }
    for reference in &pin.content.agent_profiles {
        if !agent(db, reference.id.as_str(), reference.revision, None, session).await? {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(super) async fn agent(
    db: &Database,
    id: &str,
    revision: u32,
    digest: Option<&str>,
    session: &mut ClientSession,
) -> mongodb::error::Result<bool> {
    let Some(row) = exact(db, "agent_profile_revisions", id, revision, session).await? else {
        return Ok(false);
    };
    let Ok(pin) = agent_profiles::read_revision(&row) else {
        return Ok(false);
    };
    if digest.is_some_and(|expected| expected != pin.digest) {
        return Ok(false);
    }
    unfenced(db, "agent_profiles", id, session).await
}

async fn exact(
    db: &Database,
    collection: &str,
    id: &str,
    revision: u32,
    session: &mut ClientSession,
) -> mongodb::error::Result<Option<Document>> {
    majority_collection::<Document>(db, collection)
        .find_one(doc! {
            "profile_id":id, "revision":i64::from(revision),
            "document_schema_version":DOCUMENT_SCHEMA_VERSION,
        })
        .session(session)
        .await
}

async fn unfenced(
    db: &Database,
    collection: &str,
    id: &str,
    session: &mut ClientSession,
) -> mongodb::error::Result<bool> {
    let row = majority_collection::<Document>(db, collection)
        .find_one(doc! {
            "_id":id, "document_schema_version":DOCUMENT_SCHEMA_VERSION,
            "account_erasure_fenced":{"$ne":true},
        })
        .projection(doc! {"_id":1})
        .session(session)
        .await?;
    Ok(row.is_some())
}
