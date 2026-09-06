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
) -> Result<Vec<SandboxAgentActivity>, ApplyError> {
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
        activities.push(
            SandboxAgentActivity::from_live_agent(agent, newest.as_ref()).map_err(|error| {
                ApplyError::Domain(SandboxStoreError::Corrupt(error.to_string()))
            })?,
        );
    }
    Ok(activities)
}

pub(super) async fn fence_live_agents(
    store: &MongoSandboxStore,
    agents: &[Agent],
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    for agent in agents.iter().filter(|agent| agent.status.is_live()) {
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
