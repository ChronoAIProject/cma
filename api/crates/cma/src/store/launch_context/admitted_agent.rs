//! Exact durable Agent admission discovery for a partially checkpointed Launch.
use super::{corrupt, query_error, Agent, LaunchResolutionError, MongoSandboxStore};
use crate::store::agents::read_agent;
use layer2_core::{profiles::content_digest, sandbox::AgentOrigin, trigger_domain::Launch};
use mongodb::bson::doc;

pub(super) async fn read(
    store: &MongoSandboxStore,
    launch: &Launch,
) -> Result<Option<Agent>, LaunchResolutionError> {
    let origin = AgentOrigin::Trigger {
        trigger_id: launch.trigger_id.to_string(),
        launch_id: launch.id.to_string(),
    };
    let digest = content_digest(&origin).map_err(|error| corrupt(&error.to_string()))?;
    let mut found: Option<Agent> = None;
    // Both are canonical AgentService keys: ResumeExisting uses the profile
    // key even when admission creates a new Agent instead of finding a reuse.
    for key in [
        format!("trigger:{digest}"),
        format!("trigger-profile:{digest}"),
    ] {
        if let Some(agent) = read_key(store, launch, &origin, &key).await? {
            if found
                .as_ref()
                .is_some_and(|previous| previous.id != agent.id)
            {
                return Err(corrupt("Launch admission keys identify different Agents"));
            }
            found = Some(agent);
        }
    }
    Ok(found)
}

async fn read_key(
    store: &MongoSandboxStore,
    launch: &Launch,
    origin: &AgentOrigin,
    key: &str,
) -> Result<Option<Agent>, LaunchResolutionError> {
    let receipt_id = content_digest(&(launch.owner.as_str(), key))
        .map_err(|error| corrupt(&error.to_string()))?;
    let receipt = store
        .agent_reuse_bindings
        .find_one(doc! {"_id":&receipt_id})
        .await
        .map_err(|error| LaunchResolutionError::Unavailable(error.to_string()))?;
    let query = if let Some(receipt) = &receipt {
        crate::store::sandbox_mongo::require_version(receipt).map_err(|error| corrupt(&error))?;
        let fingerprint = receipt
            .get_str("fingerprint")
            .map_err(|_| corrupt("Agent admission receipt has no fingerprint"))?;
        if receipt.get_str("_id").ok() != Some(receipt_id.as_str())
            || receipt.get_str("owner").ok() != Some(launch.owner.as_str())
            || receipt.get_str("creation_key").ok() != Some(key)
            || fingerprint.len() != 64
            || !fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(corrupt(
                "Agent admission receipt identity disagrees with its Launch",
            ));
        }
        let id = receipt
            .get_str("agent_id")
            .map_err(|_| corrupt("Agent admission receipt has no Agent"))?;
        doc! {"_id":id,"owner":launch.owner.as_str()}
    } else {
        doc! {"owner":launch.owner.as_str(),"creation_key":key}
    };
    let Some(row) = store
        .agents
        .find_one(query)
        .await
        .map_err(|error| LaunchResolutionError::Unavailable(error.to_string()))?
    else {
        return if receipt.is_some() {
            Err(corrupt("Agent admission receipt target is missing"))
        } else {
            Ok(None)
        };
    };
    let agent =
        read_agent(&row).map_err(|error| query_error("read committed Launch Agent", error))?;
    if agent.owner != launch.owner
        || launch.resolution.sandbox.id() != Some(&agent.sandbox_id)
        || launch.resolution.workspace.id() != Some(&agent.workspace_id)
        || receipt.as_ref().is_some_and(|receipt| {
            receipt.get_str("agent_id").ok() != Some(agent.id.as_str())
                || receipt.get_str("sandbox_id").ok() != Some(agent.sandbox_id.as_str())
        })
        || (receipt.is_none() && (&agent.origin != origin || agent.creation_key != key))
    {
        return Err(corrupt(
            "committed Agent lineage disagrees with its Launch admission",
        ));
    }
    Ok(Some(agent))
}
