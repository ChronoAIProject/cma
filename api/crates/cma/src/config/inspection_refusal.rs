//! Operator-only control for qualifying bound runtime inspection in staging.
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use layer2_core::sandbox::AgentWorkStep;
use layer2_core::SandboxId;

const SETTING: &str = "CHRONO_STAGING_AGENT_INSPECTION_REFUSAL";
const MAX_WINDOW: Duration = Duration::from_secs(45 * 60);

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct InspectionRefusalControl {
    sandbox_id: SandboxId,
    step: AgentWorkStep,
    expires_at: SystemTime,
}

impl InspectionRefusalControl {
    pub(super) fn from_lookup(
        lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
        now: SystemTime,
    ) -> Result<Option<Self>, String> {
        let Some(value) = lookup(SETTING)? else {
            return Ok(None);
        };
        if lookup("CHRONO_ENVIRONMENT")?.as_deref() != Some("staging") {
            return Ok(None);
        }
        Self::parse(&value, now).map(Some).ok_or_else(|| {
            format!("{SETTING} requires one exact Sandbox ID, CloseProcess or PublishConnection, and a Unix expiry no more than 45 minutes ahead")
        })
    }

    fn parse(value: &str, now: SystemTime) -> Option<Self> {
        let (sandbox_id, rest) = value.split_once(':')?;
        let id = sandbox_id.strip_prefix("sbx_")?;
        if id.len() != 32
            || !id
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return None;
        }
        let (step, expires_at) = rest.split_once(':')?;
        let step = match step {
            "CloseProcess" => AgentWorkStep::CloseProcess,
            "PublishConnection" => AgentWorkStep::PublishConnection,
            _ => return None,
        };
        let expires_at = UNIX_EPOCH.checked_add(Duration::from_secs(expires_at.parse().ok()?))?;
        if expires_at > now.checked_add(MAX_WINDOW)? {
            return None;
        }
        Some(Self {
            sandbox_id: SandboxId(sandbox_id.into()),
            step,
            expires_at,
        })
    }

    pub(crate) fn expiry_for(
        &self,
        sandbox_id: &SandboxId,
        step: AgentWorkStep,
    ) -> Option<SystemTime> {
        (&self.sandbox_id == sandbox_id && self.step == step).then_some(self.expires_at)
    }
}

#[cfg(test)]
#[path = "inspection_refusal_tests.rs"]
mod tests;
