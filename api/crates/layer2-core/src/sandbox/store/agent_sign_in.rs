use crate::profiles::ModelAccess;
use crate::sandbox::{
    Agent, AgentLogKind, AgentStatus, AgentWorkStep, Sandbox, SandboxSignInState, SandboxStatus,
};

use super::{AgentWorkMutation, SandboxStoreError};

/// Couple initial sign-in observations to an already validated agent-work publication.
pub fn apply_agent_sign_in(
    sandbox: &Sandbox,
    current: &Agent,
    mutation: &AgentWorkMutation,
) -> Result<Option<Sandbox>, SandboxStoreError> {
    let notice = matches!(
        mutation.agent_logs.as_slice(),
        [AgentLogKind::SignInRequired { .. }]
    );
    let opened = current.status == AgentStatus::Starting
        && current.work.step == AgentWorkStep::PublishReady
        && mutation.agent.status == AgentStatus::Ready;
    if !notice && !opened {
        return Ok(None);
    }
    if sandbox.id != current.sandbox_id
        || sandbox.owner != current.owner
        || sandbox.status != SandboxStatus::Active
        || sandbox.lifecycle.checkpoint_capture.is_some()
    {
        return Err(SandboxStoreError::Conflict);
    }
    if sandbox.profile.content.model_access != ModelAccess::OwnCredentials {
        return if notice {
            Err(SandboxStoreError::Conflict)
        } else {
            Ok(None)
        };
    }
    let desired = if notice {
        SandboxSignInState::Required
    } else {
        SandboxSignInState::Complete
    };
    if sandbox.credential.sign_in == desired {
        return Ok(None);
    }
    let mut next = sandbox.clone();
    next.credential.sign_in = desired;
    next.revision = next
        .revision
        .checked_add(1)
        .ok_or(SandboxStoreError::Conflict)?;
    next.updated_at = next.updated_at.max(mutation.appended_at.clone());
    Ok(Some(next))
}
