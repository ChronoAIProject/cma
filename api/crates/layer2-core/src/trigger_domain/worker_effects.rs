use serde::{Deserialize, Serialize};

use crate::ids::LaunchId;
use crate::seconds_from_rfc3339;

use super::TriggerError;

const MAX_WORKER_HOLDER_BYTES: usize = 128;
const MAX_EFFECT_VALUE_BYTES: usize = 256;
pub const MAX_LAUNCH_RETRY_DELAY_SECONDS: u64 = 15 * 60;
pub const MAX_LAUNCH_WORK_ATTEMPTS: u32 = 8;

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

    const fn accepts(self, kind: LaunchEffectKind) -> bool {
        matches!(
            (self, kind),
            (
                Self::ResolveSandbox,
                LaunchEffectKind::CreateSandbox | LaunchEffectKind::ReviveSandbox
            ) | (Self::WaitForSandbox, LaunchEffectKind::CreateSandbox)
                | (
                    Self::ResolveWorkspace | Self::WaitForWorkspace,
                    LaunchEffectKind::CreateWorkspace
                )
                | (Self::ResolveAgent, LaunchEffectKind::CreateAgent)
        )
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
}

impl LaunchEffectKind {
    pub fn intent_key(self, launch_id: &LaunchId) -> String {
        let operation = match self {
            Self::CreateSandbox => "create-sandbox",
            Self::ReviveSandbox => "revive-sandbox",
            Self::CreateWorkspace => "create-workspace",
            Self::CreateAgent => "create-agent",
        };
        format!("launch:{launch_id}:{operation}:v1")
    }

    const fn result_prefix(self) -> &'static str {
        match self {
            Self::CreateSandbox | Self::ReviveSandbox => "sbx_",
            Self::CreateWorkspace => "wks_",
            Self::CreateAgent => "agt_",
        }
    }
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
        let Some(now) = seconds_from_rfc3339(now) else {
            return false;
        };
        self.step.is_pending()
            && seconds_from_rfc3339(&self.ready_at).is_some_and(|ready| ready <= now)
            && self.lease.as_ref().is_none_or(|lease| {
                seconds_from_rfc3339(&lease.expires_at).is_some_and(|expires| expires <= now)
            })
    }

    pub fn claim(
        &mut self,
        holder: String,
        now: &str,
        expires_at: String,
    ) -> Result<u64, TriggerError> {
        let now_seconds = seconds_from_rfc3339(now);
        let expires_seconds = seconds_from_rfc3339(&expires_at);
        if !valid_text(&holder, MAX_WORKER_HOLDER_BYTES)
            || now_seconds.is_none()
            || expires_seconds.is_none()
            || expires_seconds <= now_seconds
            || !self.is_due(now)
        {
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
        if let Some(effect) = &mut self.effect {
            match effect.state {
                LaunchEffectState::Prepared | LaunchEffectState::Applied => {
                    effect.fence = fence;
                    effect.updated_at = now.to_string();
                }
                LaunchEffectState::Executing => {
                    effect.state = LaunchEffectState::RecoveryRequired;
                    effect.fence = fence;
                    effect.updated_at = now.to_string();
                }
                LaunchEffectState::RecoveryRequired => {
                    effect.fence = fence;
                    effect.updated_at = now.to_string();
                }
            }
        }
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

    pub(super) fn schedule(&mut self, step: LaunchWorkStep, ready_at: String) {
        self.step = step;
        self.ready_at = ready_at;
        self.attempts = 0;
        self.lease = None;
        self.effect = None;
    }

    pub(super) fn defer(
        &mut self,
        holder: &str,
        fence: u64,
        ready_at: String,
    ) -> Result<(), TriggerError> {
        self.require_owner(holder, fence)?;
        if self.effect.as_ref().is_some_and(|effect| {
            matches!(
                effect.state,
                LaunchEffectState::Executing | LaunchEffectState::RecoveryRequired
            )
        }) {
            return Err(TriggerError::new("invalid_launch_defer", "work.effect"));
        }
        self.ready_at = ready_at;
        self.lease = None;
        Ok(())
    }

    pub fn retry(
        &mut self,
        holder: &str,
        fence: u64,
        ready_at: String,
    ) -> Result<(), TriggerError> {
        self.require_owner(holder, fence)?;
        if self.effect.as_ref().is_some_and(|effect| {
            !matches!(
                effect.state,
                LaunchEffectState::Prepared | LaunchEffectState::RecoveryRequired
            )
        }) {
            return Err(TriggerError::new("invalid_launch_retry", "work.effect"));
        }
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
        if !self.step.accepts(kind)
            || !valid_text(&intent_key, MAX_EFFECT_VALUE_BYTES)
            || self.effect.is_some()
        {
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

    pub fn mark_effect_recovery_required(
        &mut self,
        holder: &str,
        fence: u64,
        at: String,
    ) -> Result<(), TriggerError> {
        self.require_owner(holder, fence)?;
        let effect = self
            .effect
            .as_mut()
            .filter(|effect| effect.fence == fence && effect.state == LaunchEffectState::Executing)
            .ok_or_else(|| TriggerError::new("invalid_launch_effect", "work.effect.state"))?;
        effect.state = LaunchEffectState::RecoveryRequired;
        effect.updated_at = at;
        Ok(())
    }

    pub fn stop_preserving_effect(&mut self, at: String) {
        self.step = LaunchWorkStep::None;
        self.ready_at = at.clone();
        self.lease = None;
        if let Some(effect) = &mut self.effect {
            if effect.state == LaunchEffectState::Executing {
                effect.state = LaunchEffectState::RecoveryRequired;
                effect.updated_at = at;
            }
        }
    }

    pub fn release_lease(&mut self) {
        self.lease = None;
    }

    pub fn validate(
        &self,
        launch_id: &LaunchId,
        preserve_stopped_effect: bool,
    ) -> Result<(), TriggerError> {
        if self.next_fence == 0 || seconds_from_rfc3339(&self.ready_at).is_none() {
            return Err(invalid_work());
        }
        if let Some(lease) = &self.lease {
            if !valid_text(&lease.holder, MAX_WORKER_HOLDER_BYTES)
                || lease.fence == 0
                || lease.fence >= self.next_fence
                || seconds_from_rfc3339(&lease.expires_at).is_none()
            {
                return Err(invalid_work());
            }
        }
        let Some(effect) = &self.effect else {
            return Ok(());
        };
        let result_matches_state = match effect.state {
            LaunchEffectState::Prepared
            | LaunchEffectState::Executing
            | LaunchEffectState::RecoveryRequired => effect.result_id.is_none(),
            LaunchEffectState::Applied => effect
                .result_id
                .as_deref()
                .is_some_and(|value| valid_effect_result(effect.kind, value)),
        };
        if effect.attempt == 0
            || effect.fence == 0
            || effect.fence >= self.next_fence
            || !valid_text(&effect.intent_key, MAX_EFFECT_VALUE_BYTES)
            || effect.intent_key != effect.kind.intent_key(launch_id)
            || seconds_from_rfc3339(&effect.updated_at).is_none()
            || !result_matches_state
            || !(self.step.accepts(effect.kind)
                || preserve_stopped_effect && self.step == LaunchWorkStep::None)
            || self
                .lease
                .as_ref()
                .is_some_and(|lease| lease.fence != effect.fence)
            || effect.state == LaunchEffectState::Executing && self.lease.is_none()
        {
            return Err(invalid_work());
        }
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
        if !valid_effect_result(effect.kind, &result_id) {
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

    pub fn reconcile_effect_applied(
        &mut self,
        holder: &str,
        fence: u64,
        kind: LaunchEffectKind,
        result_id: String,
        at: String,
    ) -> Result<(), TriggerError> {
        self.require_owner(holder, fence)?;
        let effect = self
            .effect
            .as_mut()
            .filter(|effect| {
                effect.fence == fence
                    && effect.kind == kind
                    && effect.state == LaunchEffectState::RecoveryRequired
            })
            .ok_or_else(|| TriggerError::new("invalid_launch_effect", "work.effect.state"))?;
        if !valid_effect_result(kind, &result_id) {
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

fn valid_text(value: &str, maximum: usize) -> bool {
    !value.trim().is_empty() && value.len() <= maximum && !value.chars().any(char::is_control)
}

fn valid_effect_result(kind: LaunchEffectKind, value: &str) -> bool {
    valid_text(value, MAX_EFFECT_VALUE_BYTES)
        && value.starts_with(kind.result_prefix())
        && value.len() > kind.result_prefix().len()
}

fn invalid_work() -> TriggerError {
    TriggerError::new("invalid_launch_work", "work")
}
