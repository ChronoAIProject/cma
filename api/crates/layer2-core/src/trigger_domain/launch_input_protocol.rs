//! Immutable admission selection; process flags never reinterpret accepted work.
use super::{Launch, LaunchService, LaunchServiceError, TriggerError};
use crate::profiles::content_digest;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Immutable public association; it carries no execution authority or input content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchFirstMessageCommand {
    pub agent_id: crate::AgentId,
    pub command_id: crate::ids::AgentCommandId,
    pub response_id: crate::ids::AgentResponseId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchInputProtocol {
    schema_version: u16,
    launch_id: crate::LaunchId,
    command_fingerprint: String,
    created_at: String,
    initiation_sha256: String,
}

impl LaunchInputProtocol {
    pub(super) fn capture(launch: &Launch) -> Result<Self, LaunchServiceError> {
        let initiation = launch
            .initiation
            .as_ref()
            .ok_or(LaunchServiceError::Forbidden)?;
        let value = Self {
            schema_version: 1,
            launch_id: launch.id.clone(),
            command_fingerprint: launch.command_fingerprint.clone(),
            created_at: launch.created_at.clone(),
            initiation_sha256: content_digest(initiation)
                .map_err(|_| LaunchServiceError::Forbidden)?,
        };
        value
            .require_launch(launch)
            .map_err(|_| LaunchServiceError::Forbidden)?;
        Ok(value)
    }

    pub fn require_launch(&self, launch: &Launch) -> Result<(), TriggerError> {
        let valid = launch.initiation.as_ref().is_some_and(|initiation| {
            matches!(
                initiation.actor(),
                crate::resource_access::ResourceActor::Human { .. }
                    | crate::resource_access::ResourceActor::Api { .. }
            ) && initiation.require_launch(launch).is_ok()
                && content_digest(initiation).ok().as_ref() == Some(&self.initiation_sha256)
        });
        if self.schema_version != 1
            || self.launch_id != launch.id
            || self.command_fingerprint != launch.command_fingerprint
            || self.created_at != launch.created_at
            || !valid
        {
            return Err(TriggerError::new(
                "invalid_launch_input_protocol",
                "input_protocol",
            ));
        }
        Ok(())
    }
}

impl LaunchService {
    /// Only new launch admission selects this protocol. Durable replay wins.
    pub fn with_common_first_messages(&self, enabled: bool) -> Arc<Self> {
        Arc::new(Self {
            common_first_messages: enabled,
            standing: self.standing.clone(),
            definitions: self.definitions.clone(),
            profile_reader: self.profile_reader.clone(),
            sandbox_profiles: self.sandbox_profiles.clone(),
            workspace_profiles: self.workspace_profiles.clone(),
            launches: self.launches.clone(),
            contexts: self.contexts.clone(),
            connections: self.connections.clone(),
            policy: self.policy.clone(),
            credentials: self.credentials.clone(),
        })
    }
}
