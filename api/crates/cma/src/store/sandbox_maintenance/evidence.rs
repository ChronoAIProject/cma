use crate::store::agent_log_telemetry::ScanCaller;
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{Agent, SandboxAgentActivity};
use mongodb::bson::doc;
use mongodb::ClientSession;

use super::super::agent_capacity_recovery::source::{capture_segmented, fence_agent};
use super::super::agent_log_integrity::{scan_complete_log_with_policy, IntegrityFailurePolicy};
use super::super::connection_recovery_activity::{self, COMPLETED};
use super::super::sandbox_mongo::{signed, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION};
use super::transaction::ApplyError;

pub(super) mod certified_legacy;
pub(super) mod legacy_log_budget;
pub(super) mod scoped;
pub(super) mod sessions;

pub(super) struct LogValidation<'a> {
    pub(super) failure_policy: IntegrityFailurePolicy,
    pub(super) certified_legacy: Option<&'a certified_legacy::CertifiedHeads>,
    pub(super) legacy_formats_checked: bool,
    pub(super) use_recovery_activity: bool,
}

// Historical legacy-only fixture entry; production requires captured sources.
#[cfg(test)]
pub(super) async fn live_activities(
    store: &MongoSandboxStore,
    agents: &[Agent],
    session: &mut ClientSession,
    observed_at: &str,
) -> Result<Vec<SandboxAgentActivity>, ApplyError> {
    live_activities_with_sources(
        store,
        agents,
        session,
        observed_at,
        None,
        LogValidation {
            failure_policy: IntegrityFailurePolicy::Quarantine,
            legacy_formats_checked: false,
            use_recovery_activity: true,
            certified_legacy: None,
        },
        false,
    )
    .await
}

pub(super) async fn live_activities_with_sources(
    store: &MongoSandboxStore,
    agents: &[Agent],
    session: &mut ClientSession,
    observed_at: &str,
    sources: Option<&scoped::CapturedActivities>,
    log_validation: LogValidation<'_>,
    sessions_already_clear: bool,
) -> Result<Vec<SandboxAgentActivity>, ApplyError> {
    let open_session = !sessions_already_clear && store
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
        let observed = sources
            .map(|sources| sources.latest(agent))
            .transpose()?
            .flatten();
        let certified = log_validation
            .certified_legacy
            .and_then(|heads| heads.get(&agent.id));
        let scoped = observed.is_some() || certified.is_some();
        let mut newest = observed.flatten();
        if !scoped {
            // This entry has no scoped proof. A v2 envelope refuses before the
            // legacy scan; it cannot silently select a lifetime fallback.
            if !log_validation.legacy_formats_checked {
                capture_segmented(store, agent, session).await?;
            }
            scan_complete_log_with_policy(
                ScanCaller::InactivityProof,
                store,
                &agent.id,
                agent.conversation.last_seq,
                session,
                log_validation.failure_policy,
                |entry| {
                    newest = Some(entry);
                    Ok(())
                },
            )
            .await?;
        }
        let mut activity = SandboxAgentActivity::from_live_agent(agent, newest.as_ref())
            .map_err(|error| ApplyError::Domain(SandboxStoreError::Corrupt(error.to_string())))?;
        if let Some(certified) = certified {
            activity.newest_log = Some(certified.clone());
        } else if let Some(entry) = newest.as_ref().filter(|entry| {
            log_validation.use_recovery_activity
                && connection_recovery_activity::is_connection_completion(entry)
        }) {
            let row = store
                .agents
                .find_one(doc! { "_id": agent.id.as_str() })
                .projection(doc! { COMPLETED: 1 })
                .session(&mut *session)
                .await
                .map_err(ApplyError::Driver)?;
            if let Some(row) = row {
                activity.newest_log = Some(connection_recovery_activity::project_head(
                    entry,
                    connection_recovery_activity::activity_time(&row, agent, entry),
                ));
            }
        }
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
        activity.open_terminal_or_desktop_session = !sessions_already_clear
            && (open_session || layer2_core::sandbox::has_open_desktop_session(&agent.sandbox_id));
        activities.push(activity);
    }
    Ok(activities)
}

/// An owner session on a Sandbox without live Agents; with Agents, each
/// activity row carries it.
pub(super) async fn open_session(
    store: &MongoSandboxStore,
    sandbox: &layer2_core::sandbox::Sandbox,
    observed_at: &str,
    session: &mut ClientSession,
) -> Result<bool, ApplyError> {
    let leased = store
        .sandbox_sessions
        .find_one(doc! {
            "sandbox_id": sandbox.id.as_str(),
            "lease_expires_at": { "$gt": observed_at },
        })
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .is_some();
    Ok(leased || layer2_core::sandbox::has_open_desktop_session(&sandbox.id))
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
        fence_agent(store, agent, "sandbox_maintenance_fence", session).await?;
    }
    Ok(())
}

/// Read and write-fence the complete Workspace set before replacing its runtime.
pub(super) async fn fence_idle_workspaces(
    store: &MongoSandboxStore,
    sandbox: &layer2_core::sandbox::Sandbox,
    session: &mut ClientSession,
) -> Result<bool, ApplyError> {
    Ok(idle_workspaces(store, sandbox, session).await?.is_some())
}

pub(super) async fn idle_workspaces(
    store: &MongoSandboxStore,
    sandbox: &layer2_core::sandbox::Sandbox,
    session: &mut ClientSession,
) -> Result<Option<Vec<layer2_core::sandbox::Workspace>>, ApplyError> {
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
            return Ok(None);
        }
        workspaces.push(workspace);
        if workspaces.len() > layer2_core::sandbox::store::MAX_SANDBOX_LIFECYCLE_AGENTS {
            return Err(ApplyError::Domain(SandboxStoreError::Conflict));
        }
    }
    for workspace in &workspaces {
        let locked = store.workspaces.update_one(crate::store::work_schema::query(doc! {
            "_id": workspace.id.as_str(), "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "sandbox_id": sandbox.id.as_str(), "revision": signed(workspace.revision, "maintenance Workspace revision").map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?,
        }), doc! { "$inc": { "sandbox_maintenance_fence": 1_i64 } }).session(&mut *session).await.map_err(ApplyError::Driver)?;
        if locked.matched_count != 1 {
            return Err(ApplyError::Domain(SandboxStoreError::Conflict));
        }
    }
    Ok(Some(workspaces))
}

/// Check queued work across the supplied Agent set, including terminal rows.
pub(super) async fn agent_commands_clear(
    store: &MongoSandboxStore,
    agents: &[Agent],
    session: &mut ClientSession,
) -> Result<bool, ApplyError> {
    let ids: Vec<_> = agents.iter().map(|agent| agent.id.as_str()).collect();
    let pending = store.agent_commands.find_one(doc! {
        "agent_id": {"$in": ids}, "status": {"$in": ["queued", "in_progress", "waiting_action"]},
    }).session(&mut *session).await.map_err(ApplyError::Driver)?.is_some();
    Ok(!pending)
}

pub(super) async fn queued_control(
    store: &MongoSandboxStore,
    id: &layer2_core::SandboxId,
    session: &mut ClientSession,
) -> Result<bool, ApplyError> {
    read_queued_control(store, id, session, false).await
}

pub(super) async fn read_queued_control(
    store: &MongoSandboxStore,
    id: &layer2_core::SandboxId,
    session: &mut ClientSession,
    snapshot_read: bool,
) -> Result<bool, ApplyError> {
    let mut find = store
        .sandbox_controls
        .find_one(doc! {"sandbox_id": id.as_str(), "queued": true})
        .projection(doc! {"_id": 1})
        .max_time(std::time::Duration::from_secs(2));
    if snapshot_read {
        find = find.read_concern(mongodb::options::ReadConcern::snapshot());
    }
    Ok(find
        .session(session)
        .await
        .map_err(ApplyError::Driver)?
        .is_some())
}
