//! Authority to reconnect restored conversations before final Sandbox activation.

use super::{
    Agent, AgentStatus, AgentWorkStep, Sandbox, SandboxImageRotationPhase,
    SandboxLifecycleOperation, SandboxRevivePhase,
};

impl Sandbox {
    /// Permit only restored connection publication and its pending skill recovery.
    /// This proof grants no new message, cleanup, or workspace writer admission.
    pub fn restored_connection_is_ready(&self, agent: &Agent) -> bool {
        let Some(operation) = self.lifecycle.operation.as_ref() else {
            return false;
        };
        self.id == agent.sandbox_id
            && self.owner == agent.owner
            && matches!(
                agent.status,
                AgentStatus::Ready | AgentStatus::Working | AgentStatus::WaitingApproval
            )
            && agent.requested_stop_reason.is_none()
            && self.lifecycle.checkpoint_capture.is_none()
            && matches!(
                agent.work.step,
                AgentWorkStep::PublishConnection | AgentWorkStep::InstallSkill
            )
            && self.status == operation.expected_status()
            && matches!(
                operation,
                SandboxLifecycleOperation::Revive {
                    phase: SandboxRevivePhase::QueuedMessageDelivery
                        | SandboxRevivePhase::PublishActive,
                    ..
                } | SandboxLifecycleOperation::ImageRotation {
                    phase: SandboxImageRotationPhase::QueuedMessageDelivery
                        | SandboxImageRotationPhase::PublishActive,
                    ..
                }
            )
            && self.credential.delivered
            && self.runtime.as_ref().is_some_and(|runtime| {
                runtime.provision_generation.is_some_and(|generation| {
                    generation > 0 && agent.conversation.runtime_generation == Some(generation)
                })
            })
            && ((self.retained_home_recovery().is_some() && self.restores_agent(agent))
                || self
                    .lifecycle
                    .checkpoints
                    .latest
                    .as_ref()
                    .is_some_and(|checkpoint| {
                        operation.checkpoint_epoch() == Some(checkpoint.epoch)
                            && checkpoint.conversations.iter().any(|conversation| {
                                conversation.agent_id == agent.id
                                    && agent.conversation.conversation_id.as_deref()
                                        == Some(conversation.conversation_id.as_str())
                            })
                    }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::lifecycle_test_support::{agent, NOW};
    use crate::sandbox::runtime_update_test_support::native;
    use crate::sandbox::SandboxRotationIdentity;

    #[test]
    fn restored_native_connection_requires_exact_generation_lineage_and_final_phase() {
        let mut sandbox = native();
        let mut agent = agent("restored", "original-conversation", 1);
        agent.sandbox_id = sandbox.id.clone();
        agent.owner = sandbox.owner.clone();
        agent.conversation.runtime_generation = Some(1);
        let identity =
            SandboxRotationIdentity::capture(&sandbox, std::slice::from_ref(&agent)).unwrap();
        sandbox
            .begin_image_rotation("image-v2".into(), identity, NOW.into())
            .unwrap();
        assert!(sandbox.retained_home_recovery().is_some());
        agent.work.step = AgentWorkStep::PublishConnection;
        assert!(!sandbox.restored_connection_is_ready(&agent));
        let Some(SandboxLifecycleOperation::ImageRotation { phase, .. }) =
            sandbox.lifecycle.operation.as_mut()
        else {
            unreachable!()
        };
        *phase = SandboxImageRotationPhase::PublishActive;
        assert!(sandbox.restored_connection_is_ready(&agent));
        for invalid in [
            "generation",
            "owner",
            "sandbox",
            "starting",
            "dispatch",
            "cleanup",
        ] {
            let mut foreign = agent.clone();
            match invalid {
                "generation" => foreign.conversation.runtime_generation = Some(2),
                "owner" => foreign.owner = crate::SubjectId("foreign".into()),
                "sandbox" => foreign.sandbox_id = crate::SandboxId("foreign".into()),
                "starting" => foreign.status = AgentStatus::Starting,
                "dispatch" => foreign.work.step = AgentWorkStep::DispatchFirstMessage,
                "cleanup" => foreign.work.step = AgentWorkStep::RemoveHome,
                _ => unreachable!(),
            }
            assert!(!sandbox.restored_connection_is_ready(&foreign), "{invalid}");
        }
    }
}
