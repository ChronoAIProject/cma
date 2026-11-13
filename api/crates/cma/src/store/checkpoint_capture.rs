//! Complete writer eligibility and row fences for checkpoint capture admission.

use crate::store::agent_log_telemetry::ScanCaller;
use futures_util::TryStreamExt;
use layer2_core::sandbox::store::checkpoint_capture::{
    validate_checkpoint_capture_quiet, validate_checkpoint_capture_work,
};
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{Agent, Sandbox, SandboxAgentActivity};
use mongodb::bson::{doc, Document};
use mongodb::{ClientSession, Collection};

use super::agent_capacity_recovery::source::{capture_segmented, fence_agent};
use super::agent_log::scan_complete_log_with_session;
use super::agent_log_integrity::TransactionLogError;
use super::lifecycle_execution::LifecycleLoadError;
use super::sandbox_mongo::{signed, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION};
use super::workspaces::read_workspace;

mod scoped;
pub(super) use scoped::validate_and_fence_begin_scoped;

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
        let captured = capture_segmented(store, agent, session)
            .await
            .map_err(log_error)?;
        let mut newest = captured
            .as_ref()
            .and_then(|source| source.latest_entry())
            .cloned();
        if captured.is_none() {
            scan_complete_log_with_session(
                ScanCaller::Checkpoint,
                store,
                &agent.id,
                agent.conversation.last_seq,
                session,
                |entry| {
                    newest = Some(entry);
                    Ok(())
                },
            )
            .await
            .map_err(log_error)?;
        }
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
        fence_agent(store, agent, "checkpoint_capture_fence", session)
            .await
            .map_err(log_error)?;
    }
    for workspace in &workspaces {
        fence_row(
            &store.workspaces,
            workspace.id.as_str(),
            workspace.revision,
            session,
        )
        .await?;
    }
    Ok(())
}

fn log_error(error: TransactionLogError) -> LifecycleLoadError {
    match error {
        TransactionLogError::Domain(error) => LifecycleLoadError::Domain(error),
        TransactionLogError::Driver(error) => LifecycleLoadError::Driver(error),
    }
}

async fn fence_row(
    collection: &Collection<Document>,
    id: &str,
    revision: u64,
    session: &mut ClientSession,
) -> Result<(), LifecycleLoadError> {
    let filter = doc! {
        "_id": id,
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "revision": signed(revision, "checkpoint capture row revision")
            .map_err(|error| LifecycleLoadError::Domain(SandboxStoreError::Corrupt(error)))?,
    };
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
