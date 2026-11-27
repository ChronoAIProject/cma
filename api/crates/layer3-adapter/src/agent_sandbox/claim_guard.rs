//! Bounded create readiness observations never grant replacement authority.
use super::{model, pods, AgentSandboxClient, PodBinding};
use crate::{AdapterError, Result, RuntimeHandle};
use k8s_openapi::api::core::v1::{Pod, PodCondition};
use kube::{api::DynamicObject, api::ListParams, ResourceExt};
use std::{future::Future, time::Duration};

mod scheduling;

pub(super) const BUDGET: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum NativeClaimGuardReason {
    #[error("readiness deadline reached after a validated pending observation")]
    Pending,
    #[error("readiness deadline reached")]
    TimedOut,
    #[error("runtime pod is unschedulable")]
    Unschedulable,
    #[error("runtime container is in CrashLoopBackOff")]
    CrashLoop,
    #[error("runtime container restarted during readiness observation")]
    Restarting,
}

pub(super) struct Sample {
    binding: Option<PodBinding>,
    pod_uid: String,
    restart_count: Option<i32>,
    crash_loop: bool,
}

/// Call only after validating this Pod's exact Sandbox owner and namespace.
pub(super) fn sample(member: &DynamicObject, pod: &Pod) -> Result<Sample> {
    let pod_uid = pod
        .uid()
        .filter(|uid| !uid.is_empty())
        .ok_or_else(|| super::rejected("guarded Pod has no UID"))?;
    if let Some(condition) = unschedulable_condition(member, pod) {
        if let Some(message) = condition.message.as_deref() {
            scheduling::report(message);
        }
        return Err(AdapterError::NativeClaimNotReady(
            NativeClaimGuardReason::Unschedulable,
        ));
    }
    let container = pod
        .status
        .as_ref()
        .and_then(|status| status.container_statuses.as_ref())
        .and_then(|containers| containers.iter().find(|value| value.name == "runtime"));
    let crash_loop = container
        .and_then(|value| value.state.as_ref())
        .and_then(|state| state.waiting.as_ref())
        .and_then(|state| state.reason.as_deref())
        == Some("CrashLoopBackOff");
    let ready = model::ready(member)
        && member.metadata.deletion_timestamp.is_none()
        && container.is_some_and(|value| {
            value.ready
                && value
                    .state
                    .as_ref()
                    .is_some_and(|state| state.running.is_some())
        });
    let binding = if ready {
        match model::pod_binding(member, pod) {
            Ok(binding) => Some(binding),
            Err(AdapterError::Unavailable(_)) => None,
            Err(error) => return Err(error),
        }
    } else {
        None
    };
    Ok(Sample {
        binding,
        pod_uid,
        restart_count: container.map(|value| value.restart_count),
        crash_loop,
    })
}

fn unschedulable_condition<'a>(member: &DynamicObject, pod: &'a Pod) -> Option<&'a PodCondition> {
    // A bound or terminating Pod can retain an earlier scheduler condition.
    // Use the condition only while this exact Pod is still awaiting placement.
    if member.metadata.deletion_timestamp.is_some()
        || pod.metadata.deletion_timestamp.is_some()
        || !pod
            .spec
            .as_ref()
            .is_some_and(|spec| spec.node_name.as_deref().is_none_or(str::is_empty))
    {
        return None;
    }
    let status = pod.status.as_ref()?;
    if status.phase.as_deref() != Some("Pending") {
        return None;
    }
    status.conditions.as_ref()?.iter().find(|condition| {
        condition.type_ == "PodScheduled"
            && condition.status == "False"
            && condition.reason.as_deref() == Some("Unschedulable")
    })
}

pub(super) async fn wait<F, Fut>(stage: &'static str, mut observe: F) -> Result<PodBinding>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<Sample>>,
{
    let readiness = super::NativeReadinessObservation::default();
    bounded(stage, &readiness, async {
        let mut previous: Option<(String, i32)> = None;
        loop {
            match readiness
                .observe(async {
                    match observe().await {
                        Ok(observed) => {
                            if observed.crash_loop {
                                return Err(AdapterError::NativeClaimNotReady(
                                    NativeClaimGuardReason::CrashLoop,
                                ));
                            }
                            if let Some(count) = observed.restart_count {
                                if previous.as_ref().is_some_and(|(uid, prior)| {
                                    uid == &observed.pod_uid && count > *prior
                                }) {
                                    return Err(AdapterError::NativeClaimNotReady(
                                        NativeClaimGuardReason::Restarting,
                                    ));
                                }
                                previous = Some((observed.pod_uid, count));
                            }
                            if let Some(binding) = observed.binding {
                                return Ok(Some(binding));
                            }
                            Ok(None)
                        }
                        Err(error) => Err(error),
                    }
                })
                .await
            {
                Ok(Some(binding)) => return Ok(binding),
                Ok(None) | Err(AdapterError::Unavailable(_)) => {}
                Err(error) => return Err(error),
            }
            tokio::time::sleep(POLL).await;
        }
    })
    .await
}

/// Read-only waits can be cancelled without losing an effect acknowledgement.
pub(super) async fn bounded<T>(
    stage: &'static str,
    readiness: &super::NativeReadinessObservation,
    observe: impl Future<Output = Result<T>>,
) -> Result<T> {
    let mut metric = ObservationMetric::new(stage);
    let result = tokio::time::timeout(BUDGET, observe)
        .await
        .unwrap_or_else(|_| Err(readiness.timeout()));
    metric.outcome = match &result {
        Ok(_) => "ready",
        Err(AdapterError::NativeClaimNotReady(NativeClaimGuardReason::Pending)) => "pending",
        Err(AdapterError::NativeClaimNotReady(NativeClaimGuardReason::TimedOut)) => "timeout",
        Err(AdapterError::NativeClaimNotReady(NativeClaimGuardReason::Unschedulable)) => {
            "unschedulable"
        }
        Err(AdapterError::NativeClaimNotReady(NativeClaimGuardReason::CrashLoop)) => "crash_loop",
        Err(AdapterError::NativeClaimNotReady(NativeClaimGuardReason::Restarting)) => "restarting",
        Err(_) => "error",
    };
    if let Err(AdapterError::NativeClaimNotReady(reason)) = &result {
        tracing::warn!(stage, reason = %reason, "Native create readiness deferred; retaining assigned identity");
    }
    result
}

struct ObservationMetric {
    started: std::time::Instant,
    stage: &'static str,
    outcome: &'static str,
}

impl ObservationMetric {
    fn new(stage: &'static str) -> Self {
        Self {
            started: std::time::Instant::now(),
            stage,
            outcome: "cancelled",
        }
    }
}

impl Drop for ObservationMetric {
    fn drop(&mut self) {
        metrics::counter!("cma_native_claim_guard_total", "stage" => self.stage, "outcome" => self.outcome).increment(1);
        metrics::histogram!("cma_native_claim_guard_seconds", "stage" => self.stage, "outcome" => self.outcome).record(self.started.elapsed().as_secs_f64());
    }
}

impl AgentSandboxClient {
    pub(crate) async fn wait_ready_for_create(
        &self,
        runtime: &RuntimeHandle,
    ) -> Result<PodBinding> {
        if !self.config.native_claim_guard_enabled {
            return self.wait_ready(runtime, Duration::from_secs(180)).await;
        }
        wait("bind", || async {
            let (name, _) = model::parse_handle(runtime)?;
            let parameters = ListParams::default().labels(&pods::selector(name)).limit(3);
            let (member, page) = tokio::join!(self.get(runtime), self.pods.list(&parameters));
            let member = member?.ok_or_else(|| super::unavailable("Sandbox no longer exists"))?;
            if member.name_any() != name {
                return Err(super::rejected("guarded Sandbox name changed"));
            }
            let pod = self.owned_pod_from_page(&member, page.map_err(super::api_error)?)?;
            sample(&member, &pod)
        })
        .await
    }
}

#[cfg(test)]
mod tests;
