use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{Agent, SandboxAgentActivity};
use mongodb::bson::doc;
use mongodb::ClientSession;

use super::super::agent_log::scan_complete_log_with_session;
use super::super::sandbox_mongo::{signed, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION};
use super::transaction::ApplyError;

pub(super) async fn live_activities(
    store: &MongoSandboxStore,
    agents: &[Agent],
    session: &mut ClientSession,
    observed_at: &str,
) -> Result<Vec<SandboxAgentActivity>, ApplyError> {
    let open_session = store
        .sandbox_sessions
        .find_one(doc! {
            "sandbox_id": agents.first().map(|agent| agent.sandbox_id.as_str()).unwrap_or_default(),
            "lease_expires_at": { "$gt": observed_at },
        })
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .is_some();
    let mut activities = Vec::new();
    for agent in agents.iter().filter(|agent| agent.status.is_live()) {
        let mut newest = None;
        scan_complete_log_with_session(
            store,
            &agent.id,
            agent.conversation.last_seq,
            session,
            |entry| {
                newest = Some(entry);
                Ok(())
            },
        )
        .await?;
        let mut activity = SandboxAgentActivity::from_live_agent(agent, newest.as_ref())
            .map_err(|error| ApplyError::Domain(SandboxStoreError::Corrupt(error.to_string())))?;
        // A durable command receipt may arrive before the Agent checkpoint.
        // Keep every nonterminal UI/API/channel/trigger command visible to the
        // inactivity proof so admitted work cannot be paused as idle.
        activity.external_activity_present = store
            .agent_commands
            .find_one(doc! {
                "agent_id": agent.id.as_str(),
                "status": {
                    "$in": ["queued", "in_progress", "waiting_action"]
                },
            })
            .session(&mut *session)
            .await
            .map_err(ApplyError::Driver)?
            .is_some();
        activity.open_terminal_or_desktop_session =
            open_session || layer2_core::sandbox::has_open_desktop_session(&agent.sandbox_id);
        activities.push(activity);
    }
    Ok(activities)
}

pub(super) async fn fence_live_agents(
    store: &MongoSandboxStore,
    agents: &[Agent],
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    let live: Vec<_> = agents
        .iter()
        .filter(|agent| agent.status.is_live())
        .cloned()
        .collect();
    fence_agents(store, &live, session).await
}

pub(super) async fn fence_agents(
    store: &MongoSandboxStore,
    agents: &[Agent],
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    for agent in agents {
        let locked = store
            .agents
            .update_one(
                doc! {
                    "_id": agent.id.as_str(),
                    "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                    "sandbox_id": agent.sandbox_id.as_str(),
                    "revision": signed(agent.revision, "maintenance Agent revision")
                        .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?,
                    "last_seq": signed(agent.conversation.last_seq, "maintenance Agent log sequence")
                        .map_err(|_| ApplyError::Domain(SandboxStoreError::SequenceConflict))?,
                },
                doc! { "$inc": { "sandbox_maintenance_fence": 1_i64 } },
            )
            .session(&mut *session)
            .await
            .map_err(ApplyError::Driver)?;
        if locked.matched_count != 1 {
            return Err(ApplyError::Domain(SandboxStoreError::Conflict));
        }
    }
    Ok(())
}

/// Read and write-fence the complete Workspace set before replacing its runtime.
pub(super) async fn fence_idle_workspaces(
    store: &MongoSandboxStore,
    sandbox: &layer2_core::sandbox::Sandbox,
    session: &mut ClientSession,
) -> Result<bool, ApplyError> {
    use futures_util::TryStreamExt;
    let mut cursor = store
        .workspaces
        .find(doc! { "sandbox_id": sandbox.id.as_str() })
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?;
    let mut workspaces = Vec::new();
    while let Some(document) = cursor
        .stream(&mut *session)
        .try_next()
        .await
        .map_err(ApplyError::Driver)?
    {
        let workspace =
            super::super::workspaces::read_workspace(&document).map_err(ApplyError::Domain)?;
        if workspace.sandbox_id != sandbox.id || workspace.owner != sandbox.owner {
            return Err(ApplyError::Domain(SandboxStoreError::Conflict));
        }
        if workspace.work.step.is_pending() || workspace.work.lease.is_some() {
            return Ok(false);
        }
        workspaces.push(workspace);
    }
    for workspace in workspaces {
        let locked = store.workspaces.update_one(doc! {
            "_id": workspace.id.as_str(), "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "sandbox_id": sandbox.id.as_str(), "revision": signed(workspace.revision, "maintenance Workspace revision").map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?,
        }, doc! { "$inc": { "sandbox_maintenance_fence": 1_i64 } }).session(&mut *session).await.map_err(ApplyError::Driver)?;
        if locked.matched_count != 1 {
            return Err(ApplyError::Domain(SandboxStoreError::Conflict));
        }
    }
    Ok(true)
}
