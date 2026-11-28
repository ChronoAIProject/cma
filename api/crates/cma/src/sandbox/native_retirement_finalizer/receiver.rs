use super::*;
use crate::sandbox::live_agent_stopper;
use crate::store::native_retirement_action::RetirementObservation;
use layer2_core::sandbox::Agent;
use layer3_adapter::owner_protocol::{TransportOwner, OWNER_PROTOCOL_VERSION};
use layer3_adapter::receiver_failure::RetirementReceiver;

impl NativeRetirementFinalizer {
    async fn admit_unreachable(
        &self,
        parent: &Sandbox,
        provider: &dyn RuntimeRetirement,
        request: &RuntimeAdoptionRequest,
        children: &[Agent],
    ) -> Result<Option<RetirementAction>, SandboxStoreError> {
        let Some(child) = children.iter().find(|child| {
            child.status.is_live()
                && !child.has_no_runtime_effects()
                && child
                    .conversation
                    .runtime_generation
                    .is_some_and(|generation| generation > 0)
        }) else {
            return Ok(None);
        };
        let lease = parent
            .work
            .lease
            .as_ref()
            .ok_or(SandboxStoreError::Conflict)?;
        // Challenge only asks whether the endpoint can negotiate. No confirmation
        // or lease mutation uses this correlation from the retiring parent claim.
        let owner = TransportOwner {
            protocol: OWNER_PROTOCOL_VERSION,
            agent_id: child.id.to_string(),
            instance_id: lease.holder.clone(),
            epoch: lease.fence,
        };
        let Ok(Ok(Some(pod))) = tokio::time::timeout(
            Duration::from_secs(15),
            provider.probe_unreachable_receiver(request, &owner),
        )
        .await
        else {
            return Ok(None);
        };
        if !pod.is_uid_only() || !self.current(parent, &lease.holder).await? {
            return Ok(None);
        }
        let observation = RetirementObservation {
            owner: parent.owner.clone(),
            binding: parent.runtime.clone().ok_or(SandboxStoreError::Conflict)?,
            receiver: RetirementReceiver {
                runtime: request.handle.clone(),
                pod,
                owner,
            },
        };
        // Reuse the journal's admission transaction. It fences the parent claim,
        // exact observation and complete child inventory before the provider effect.
        self.records.note(parent, &observation).await?;
        let Some((stored, fences)) = self.records.observation(&parent.id).await? else {
            return Err(SandboxStoreError::Conflict);
        };
        if stored != observation {
            return Err(SandboxStoreError::Conflict);
        }
        let action = RetirementAction::new(parent, observation, children, fences)?;
        self.records.admit(parent, &action).await?;
        tracing::info!(
            sandbox_id = %parent.id,
            pod_uid = %action.observation.receiver.pod.uid,
            "Retirement admitted for an unreachable native receiver"
        );
        Ok(Some(action))
    }

    pub(in crate::sandbox) async fn offer_capture(
        &self,
        parent: &Sandbox,
    ) -> Result<(), SandboxStoreError> {
        let Some(capture) = self
            .capture
            .as_ref()
            .filter(|_| self.capture_admission.is_some())
        else {
            return Ok(());
        };
        let children = live_agent_stopper::retirement_agents(self.store.as_ref(), parent)
            .await
            .map_err(|_| SandboxStoreError::Conflict)?;
        for child in children.iter().filter(|child| {
            child.status.is_live()
                && !child.has_no_runtime_effects()
                && child
                    .conversation
                    .runtime_generation
                    .is_some_and(|generation| generation > 0)
        }) {
            if self
                .capture_admission
                .as_ref()
                .is_some_and(|admission| admission.is_closed())
            {
                break;
            }
            capture.offer(parent, child).await?;
        }
        Ok(())
    }

    pub(super) async fn admit_action(
        &self,
        sandbox: &Sandbox,
        provider: &dyn RuntimeRetirement,
        request: &RuntimeAdoptionRequest,
    ) -> Result<Option<RetirementAction>, SandboxStoreError> {
        let legacy = if self.capture_admission.is_none() && !self.probe_unreachable {
            self.records
                .observation(&sandbox.id)
                .await?
                .filter(|(observation, _)| {
                    observation.matches(sandbox) && !observation.receiver.pod.is_uid_only()
                })
        } else {
            None
        };
        let capture = self.capture.as_ref();
        if capture.is_none() && legacy.is_none() && !self.probe_unreachable {
            return Ok(None);
        }
        let children = live_agent_stopper::retirement_agents(self.store.as_ref(), sandbox)
            .await
            .map_err(|_| SandboxStoreError::Conflict)?;
        if self.probe_unreachable {
            if let Some(action) = self
                .admit_unreachable(sandbox, provider, request, &children)
                .await?
            {
                return Ok(Some(action));
            }
            if !self.runtime_identity_unchanged(sandbox, request).await {
                return Ok(None);
            }
        }
        let captured = match capture {
            Some(capture) => capture
                .observed(sandbox, &children)
                .await?
                .map(|observed| (capture, observed)),
            None => None,
        };
        let Some((observation, fences)) = captured
            .as_ref()
            .map(|(_, observed)| observed.observation())
            .or(legacy)
        else {
            return Ok(None);
        };
        let action = RetirementAction::new(sandbox, observation, &children, fences)?;
        if probe(provider, request, &action).await != Some(true) {
            return Ok(None);
        }
        match captured {
            Some((capture, observed)) => capture.admit(sandbox, &action, &observed).await?,
            None => self.records.admit(sandbox, &action).await?,
        }
        Ok(Some(action))
    }
}
