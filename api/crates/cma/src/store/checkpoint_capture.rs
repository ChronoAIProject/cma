//! Complete writer eligibility and row fences for checkpoint capture admission.

use futures_util::TryStreamExt;
use layer2_core::sandbox::store::checkpoint_capture::{
    validate_checkpoint_capture_quiet, validate_checkpoint_capture_work,
};
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{Agent, Sandbox, SandboxAgentActivity};
use mongodb::bson::{doc, Document};
use mongodb::{ClientSession, Collection};

use super::agent_log_integrity::TransactionLogError;
use super::lifecycle_execution::LifecycleLoadError;
use super::sandbox_mongo::{signed, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION};
use super::workspaces::read_workspace;

pub(super) async fn validate_and_fence_begin(
    store: &MongoSandboxStore,
    current: &Sandbox,
    next: &Sandbox,
    agents: &[Agent],
    session: &mut ClientSession,
) -> Result<(), LifecycleLoadError> {
    if current.lifecycle.checkpoint_capture.is_some() || next.lifecycle.checkpoint_capture.is_none()
    {
        return Ok(());
    }
    let mut cursor = store
        .workspaces
        .find(doc! { "sandbox_id": current.id.as_str() })
        .sort(doc! { "_id": 1 })
        .session(&mut *session)
        .await
        .map_err(LifecycleLoadError::Driver)?;
    let documents = cursor
        .stream(&mut *session)
        .try_collect::<Vec<_>>()
        .await
        .map_err(LifecycleLoadError::Driver)?;
    let workspaces = documents
        .iter()
        .map(read_workspace)
        .collect::<Result<Vec<_>, _>>()
        .map_err(LifecycleLoadError::Domain)?;
    validate_checkpoint_capture_work(current, agents, &workspaces)
        .map_err(LifecycleLoadError::Domain)?;

    let mut activities = Vec::new();
    for agent in agents {
        let newest = super::agent_log_context::validate(store, agent, session, false)
            .await
            .map_err(|error| match error {
                TransactionLogError::Domain(error) => LifecycleLoadError::Domain(error),
                TransactionLogError::Driver(error) => LifecycleLoadError::Driver(error),
            })?
            .last;
        if agent.status.is_live() {
            activities.push(
                SandboxAgentActivity::from_live_agent(agent, newest.as_ref()).map_err(|error| {
                    LifecycleLoadError::Domain(SandboxStoreError::Corrupt(error.to_string()))
                })?,
            );
        }
    }
    validate_checkpoint_capture_quiet(current, &activities, &next.updated_at)
        .map_err(LifecycleLoadError::Domain)?;

    // Rows fence callbacks and schedulers; the Sandbox write fences all new writer claims.
    for agent in agents {
        fence_row(
            &store.agents,
            agent.id.as_str(),
            agent.revision,
            Some(agent.conversation.last_seq),
            session,
        )
        .await?;
    }
    for workspace in &workspaces {
        fence_row(
            &store.workspaces,
            workspace.id.as_str(),
            workspace.revision,
            None,
            session,
        )
        .await?;
    }
    Ok(())
}

async fn fence_row(
    collection: &Collection<Document>,
    id: &str,
    revision: u64,
    last_seq: Option<u64>,
    session: &mut ClientSession,
) -> Result<(), LifecycleLoadError> {
    let mut filter = doc! {
        "_id": id,
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "revision": signed(revision, "checkpoint capture row revision")
            .map_err(|error| LifecycleLoadError::Domain(SandboxStoreError::Corrupt(error)))?,
    };
    if let Some(last_seq) = last_seq {
        filter.insert(
            "last_seq",
            signed(last_seq, "checkpoint capture Agent log head")
                .map_err(|error| LifecycleLoadError::Domain(SandboxStoreError::Corrupt(error)))?,
        );
    }
    let fenced = collection
        .update_one(
            filter,
            doc! { "$inc": { "checkpoint_capture_fence": 1_i64 } },
        )
        .session(session)
        .await
        .map_err(LifecycleLoadError::Driver)?;
    if fenced.matched_count == 1 {
        Ok(())
    } else {
        Err(LifecycleLoadError::Domain(SandboxStoreError::Conflict))
    }
}
