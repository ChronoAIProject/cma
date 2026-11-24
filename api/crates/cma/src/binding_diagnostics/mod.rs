//! Bounded read-time observations; peer requests never admit runtime work.
mod authentication;
mod transport;

use futures_util::{stream, StreamExt};
use layer2_core::{
    agent_ownership::{Member, PROTOCOL_VERSION},
    binding_diagnostics::{
        BindingDiagnosticsError, BindingHoldOverview, BindingHoldReport, BindingHoldSample,
        BindingHoldSource, HoldPhase, MAX_HOLDS_PER_REPORTER, MAX_REPORTERS,
    },
    seconds_from_rfc3339, SandboxId, SubjectId,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};
use tokio::sync::Semaphore;

pub(crate) const PATH: &str = "/internal/agent-manager/v1/binding-holds";
const MAX_BODY: usize = 256 * 1024;
const BUDGET: Duration = Duration::from_secs(5);

pub(crate) trait LocalSource: Send + Sync {
    fn sample(&self) -> BindingHoldSample;
}

pub(crate) struct MemberPage {
    pub members: Vec<Member>,
    pub truncated: bool,
}

#[async_trait::async_trait]
pub(crate) trait MembershipSource: Send + Sync {
    async fn list(&self) -> Result<MemberPage, BindingDiagnosticsError>;
    async fn check(&self, sender: &str, recipient: &str) -> Result<(), BindingDiagnosticsError>;
    async fn current_owners(
        &self,
        ids: &[SandboxId],
    ) -> Result<BTreeMap<SandboxId, SubjectId>, BindingDiagnosticsError>;
}

pub(crate) struct BindingDiagnostics {
    instance: String,
    ready: Box<dyn Fn() -> bool + Send + Sync>,
    membership: Arc<dyn MembershipSource>,
    local: Arc<dyn LocalSource>,
    authentication: authentication::Authentication,
    client: reqwest::Client,
    slots: Semaphore,
    peer_slots: Semaphore,
}

impl BindingDiagnostics {
    pub(crate) fn new(
        manager: Arc<crate::agent_manager::AgentManager>,
        membership: Arc<dyn MembershipSource>,
        local: Arc<dyn LocalSource>,
        key: ring::hmac::Key,
    ) -> Result<Arc<Self>, String> {
        Self::assemble(
            manager.coordinator.instance_id.clone(),
            Box::new(move || manager.is_ready()),
            membership,
            local,
            key,
        )
    }

    fn assemble(
        instance: String,
        ready: Box<dyn Fn() -> bool + Send + Sync>,
        membership: Arc<dyn MembershipSource>,
        local: Arc<dyn LocalSource>,
        key: ring::hmac::Key,
    ) -> Result<Arc<Self>, String> {
        if !valid_text(&instance, 128) {
            return Err("binding observation instance is invalid".into());
        }
        Ok(Arc::new(Self {
            instance,
            ready,
            membership,
            local,
            authentication: authentication::Authentication::new(key),
            client: transport::client()?,
            slots: Semaphore::new(2),
            peer_slots: Semaphore::new(4),
        }))
    }

    pub(crate) fn router(self: &Arc<Self>) -> axum::Router {
        axum::Router::new()
            .route(PATH, axum::routing::get(transport::serve))
            .with_state(self.clone())
    }

    async fn collect(
        &self,
        principal: &SubjectId,
    ) -> Result<BindingHoldOverview, BindingDiagnosticsError> {
        // Leave a bounded final interval for current owner checks and projection.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
        let mut page = self.membership.list().await?;
        if page.members.len() > MAX_REPORTERS {
            return Err(BindingDiagnosticsError::Unavailable);
        }
        let ids: BTreeSet<_> = page
            .members
            .iter()
            .map(|member| &member.instance_id)
            .collect();
        if ids.len() != page.members.len() {
            return Err(BindingDiagnosticsError::Unavailable);
        }
        self.membership
            .check(&self.instance, &self.instance)
            .await?;
        let registered_reporters = page.members.len();
        let mut observations = stream::iter(page.members.drain(..).enumerate().map(
            |(index, member)| async move {
                let result = tokio::time::timeout_at(deadline, async {
                    if member.draining
                        || member.protocol != PROTOCOL_VERSION
                        || !valid_text(&member.instance_id, 128)
                    {
                        return Err(());
                    }
                    let sample = if member.instance_id == self.instance {
                        let sample = self.local.sample();
                        valid_sample(&sample)?;
                        sample
                    } else {
                        transport::read(self, &member, principal.as_str()).await?
                    };
                    self.membership
                        .check(&self.instance, &member.instance_id)
                        .await
                        .map_err(|_| ())?;
                    Ok(BindingHoldReport {
                        reporter: format!("reporter-{}", index + 1),
                        sample,
                    })
                })
                .await;
                result.ok().and_then(Result::ok)
            },
        ))
        .buffer_unordered(4);
        let mut reports = Vec::new();
        while let Some(report) = observations.next().await {
            if let Some(report) = report {
                reports.push(report);
            }
        }
        if !(self.ready)() {
            return Err(BindingDiagnosticsError::Unavailable);
        }
        let ids: Vec<_> = reports
            .iter()
            .flat_map(|report| &report.sample.holds)
            .map(|hold| hold.sandbox_id.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let owners = self.membership.current_owners(&ids).await?;
        for report in &mut reports {
            let previous = report.sample.holds.len();
            report
                .sample
                .holds
                .retain(|hold| owners.get(&hold.sandbox_id) == Some(&hold.owner));
            report.sample.truncated |= previous != report.sample.holds.len();
        }
        if !(self.ready)() {
            return Err(BindingDiagnosticsError::Unavailable);
        }
        self.membership
            .check(&self.instance, &self.instance)
            .await?;
        reports.sort_by(|left, right| left.reporter.cmp(&right.reporter));
        bound_reports(&mut reports)?;
        let observed_sandboxes = reports
            .iter()
            .flat_map(|report| &report.sample.holds)
            .map(|hold| &hold.sandbox_id)
            .collect::<BTreeSet<_>>()
            .len();
        let overview = BindingHoldOverview {
            observed_at: layer2_core::now_rfc3339(),
            observed_sandboxes,
            registered_reporters,
            unavailable_reporters: registered_reporters.saturating_sub(reports.len()),
            truncated: page.truncated || reports.iter().any(|report| report.sample.truncated),
            reports,
        };
        if serde_json::to_vec(&overview)
            .map_err(|_| BindingDiagnosticsError::Unavailable)?
            .len()
            > MAX_BODY
        {
            return Err(BindingDiagnosticsError::Unavailable);
        }
        Ok(overview)
    }
}

#[async_trait::async_trait]
impl BindingHoldSource for BindingDiagnostics {
    async fn observe(
        &self,
        principal: &SubjectId,
    ) -> Result<BindingHoldOverview, BindingDiagnosticsError> {
        if !(self.ready)() || !valid_text(principal.as_str(), 512) {
            return Err(BindingDiagnosticsError::Unavailable);
        }
        let _slot = self
            .slots
            .try_acquire()
            .map_err(|_| BindingDiagnosticsError::Unavailable)?;
        tokio::time::timeout(BUDGET, self.collect(principal))
            .await
            .map_err(|_| BindingDiagnosticsError::Unavailable)?
    }
}

fn valid_text(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}

fn bound_reports(reports: &mut [BindingHoldReport]) -> Result<(), BindingDiagnosticsError> {
    let mut bytes = 1024 + 256 * reports.len();
    let mut full = false;
    for report in reports {
        let mut bounded = Vec::new();
        for hold in std::mem::take(&mut report.sample.holds) {
            let size = serde_json::to_vec(&hold)
                .map_err(|_| BindingDiagnosticsError::Unavailable)?
                .len()
                + 1;
            if full || bytes + size > MAX_BODY {
                report.sample.truncated = true;
                full = true;
            } else {
                bytes += size;
                bounded.push(hold);
            }
        }
        report.sample.holds = bounded;
    }
    Ok(())
}

fn valid_sample(sample: &BindingHoldSample) -> Result<(), ()> {
    if sample.observed_at.len() > 64 {
        return Err(());
    }
    let observed = seconds_from_rfc3339(&sample.observed_at).ok_or(())?;
    let now = authentication::now_ms()? / 1000;
    if observed > now || now - observed > 5 || sample.holds.len() > MAX_HOLDS_PER_REPORTER {
        return Err(());
    }
    let mut ids = BTreeSet::new();
    for hold in &sample.holds {
        if hold.first_held_at.len() > 64
            || hold
                .retry_not_before
                .as_ref()
                .is_some_and(|value| value.len() > 64)
        {
            return Err(());
        }
        let first = seconds_from_rfc3339(&hold.first_held_at).ok_or(())?;
        let retry = hold
            .retry_not_before
            .as_ref()
            .map(|value| seconds_from_rfc3339(value).ok_or(()))
            .transpose()?;
        if !valid_text(hold.sandbox_id.as_str(), 256)
            || !valid_text(hold.owner.as_str(), 512)
            || !ids.insert(&hold.sandbox_id)
            || hold.attempts == 0
            || first > observed
            || retry.is_some_and(|time| time > observed.saturating_add(301))
            || matches!(
                (&hold.phase, retry),
                (HoldPhase::Retrying, Some(_)) | (HoldPhase::Deferred, None)
            )
        {
            return Err(());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
