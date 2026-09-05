use serde::{Deserialize, Serialize};

use super::TriggerError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchWorkStep {
    ResolveSandbox,
    WaitForSandbox,
    ResolveWorkspace,
    WaitForWorkspace,
    ResolveAgent,
    WaitForAgent,
    DeliverFirstMessage,
    WaitForFirstMessage,
    Finish,
    None,
}

impl LaunchWorkStep {
    pub const fn is_pending(self) -> bool {
        !matches!(self, Self::None)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchWorkLease {
    pub holder: String,
    pub fence: u64,
    pub expires_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchEffectKind {
    CreateSandbox,
    ReviveSandbox,
    CreateWorkspace,
    CreateAgent,
    DeliverFirstMessage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchEffectState {
    Prepared,
    Executing,
    Applied,
    RecoveryRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchEffectRecord {
    pub kind: LaunchEffectKind,
    pub state: LaunchEffectState,
    pub intent_key: String,
    pub attempt: u32,
    pub fence: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_id: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchWorkState {
    pub step: LaunchWorkStep,
    pub ready_at: String,
    pub next_fence: u64,
    pub attempts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease: Option<LaunchWorkLease>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<LaunchEffectRecord>,
}

impl LaunchWorkState {
    pub fn new(step: LaunchWorkStep, ready_at: String) -> Self {
        Self {
            step,
            ready_at,
            next_fence: 1,
            attempts: 0,
            lease: None,
            effect: None,
        }
    }

    pub fn is_due(&self, now: &str) -> bool {
        self.step.is_pending()
            && self.ready_at.as_str() <= now
            && self
                .lease
                .as_ref()
                .is_none_or(|lease| lease.expires_at.as_str() <= now)
    }

    pub fn claim(
        &mut self,
        holder: String,
        now: &str,
        expires_at: String,
    ) -> Result<u64, TriggerError> {
        if holder.trim().is_empty() || expires_at.as_str() <= now || !self.is_due(now) {
            return Err(TriggerError::new("launch_lease_unavailable", "work.lease"));
        }
        let fence = self.next_fence;
        self.next_fence = fence
            .checked_add(1)
            .ok_or_else(|| TriggerError::new("launch_fence_overflow", "work.next_fence"))?;
        self.lease = Some(LaunchWorkLease {
            holder,
            fence,
            expires_at,
        });
        Ok(fence)
    }

    pub fn owns(&self, holder: &str, fence: u64) -> bool {
        self.lease
            .as_ref()
            .is_some_and(|lease| lease.holder == holder && lease.fence == fence)
    }

    pub fn advance(
        &mut self,
        holder: &str,
        fence: u64,
        step: LaunchWorkStep,
        ready_at: String,
    ) -> Result<(), TriggerError> {
        self.require_owner(holder, fence)?;
        self.step = step;
        self.ready_at = ready_at;
        self.attempts = 0;
        self.lease = None;
        self.effect = None;
        Ok(())
    }

    pub fn retry(
        &mut self,
        holder: &str,
        fence: u64,
        ready_at: String,
    ) -> Result<(), TriggerError> {
        self.require_owner(holder, fence)?;
        self.attempts = self
            .attempts
            .checked_add(1)
            .ok_or_else(|| TriggerError::new("launch_attempt_overflow", "work.attempts"))?;
        self.ready_at = ready_at;
        self.lease = None;
        Ok(())
    }

    pub fn prepare_effect(
        &mut self,
        holder: &str,
        fence: u64,
        kind: LaunchEffectKind,
        intent_key: String,
        at: String,
    ) -> Result<(), TriggerError> {
        self.require_owner(holder, fence)?;
        if intent_key.trim().is_empty() || self.effect.is_some() {
            return Err(TriggerError::new("invalid_launch_effect", "work.effect"));
        }
        self.effect = Some(LaunchEffectRecord {
            kind,
            state: LaunchEffectState::Prepared,
            intent_key,
            attempt: self.attempts.saturating_add(1),
            fence,
            result_id: None,
            updated_at: at,
        });
        Ok(())
    }

    pub fn mark_effect_executing(
        &mut self,
        holder: &str,
        fence: u64,
        at: String,
    ) -> Result<(), TriggerError> {
        self.require_owner(holder, fence)?;
        let effect = self
            .effect
            .as_mut()
            .filter(|effect| effect.fence == fence && effect.state == LaunchEffectState::Prepared)
            .ok_or_else(|| TriggerError::new("invalid_launch_effect", "work.effect.state"))?;
        effect.state = LaunchEffectState::Executing;
        effect.updated_at = at;
        Ok(())
    }

    pub fn mark_effect_applied(
        &mut self,
        holder: &str,
        fence: u64,
        result_id: String,
        at: String,
    ) -> Result<(), TriggerError> {
        self.require_owner(holder, fence)?;
        let effect = self
            .effect
            .as_mut()
            .filter(|effect| effect.fence == fence && effect.state == LaunchEffectState::Executing)
            .ok_or_else(|| TriggerError::new("invalid_launch_effect", "work.effect.state"))?;
        if result_id.trim().is_empty() {
            return Err(TriggerError::new(
                "invalid_launch_effect",
                "work.effect.result_id",
            ));
        }
        effect.state = LaunchEffectState::Applied;
        effect.result_id = Some(result_id);
        effect.updated_at = at;
        Ok(())
    }

    fn require_owner(&self, holder: &str, fence: u64) -> Result<(), TriggerError> {
        self.owns(holder, fence)
            .then_some(())
            .ok_or_else(|| TriggerError::new("stale_launch_fence", "work.lease"))
    }
}
