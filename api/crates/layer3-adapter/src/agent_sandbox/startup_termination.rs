//! Exact owned Pod evidence for startup failure detection. Identities retain the
//! same unmodified containerID format used by model::pod_binding.
use super::{model, pods, AgentSandboxClient, PodBinding};
use crate::runtime_startup::{StartupHistory, StartupIncarnation, StartupTermination};
use crate::RuntimeStartupFailureClass;
use k8s_openapi::api::core::v1::Pod;
use kube::{api::DynamicObject, api::ListParams, ResourceExt};

pub(crate) fn startup_boot_identity(binding: &PodBinding) -> &str {
    &binding.pod_uid
}

pub(crate) fn startup_incarnation(binding: &PodBinding) -> Option<StartupIncarnation> {
    Some(StartupIncarnation {
        boot: binding.pod_uid.clone(),
        process: binding
            .runtime_container_id
            .clone()
            .filter(|id| !id.is_empty())?,
        restart: binding.runtime_restart_count?.try_into().ok()?,
    })
}

impl AgentSandboxClient {
    pub(crate) fn runtime_startup_breaker_enabled(&self) -> bool {
        self.config.runtime_startup_breaker_enabled
    }

    // Outer None means the optional read could not prove identity: leave the
    // breaker untouched. Some(None) is a valid read with no termination evidence.
    pub(crate) async fn startup_termination(
        &self,
        expected: &PodBinding,
    ) -> Option<Option<StartupTermination>> {
        Some(termination(&self.owned_startup_pod(expected).await?))
    }

    pub(crate) async fn startup_history(&self, expected: &PodBinding) -> Option<StartupHistory> {
        startup_history(&self.owned_startup_pod(expected).await?, expected)
    }

    async fn owned_startup_pod(&self, expected: &PodBinding) -> Option<Pod> {
        let pod = self.pods.get(&expected.pod_name).await.ok()?;
        if pod.name_any() != expected.pod_name
            || pod.uid().as_deref() != Some(&expected.pod_uid)
            || !model::owned_pod(
                &pod,
                &self.config,
                &expected.sandbox_name,
                &expected.sandbox_uid,
            )
        {
            return None;
        }
        Some(pod)
    }

    // Used only while open: establish a replacement boot without waiting for Ready.
    // A ready binding also preserves the acknowledged-assignment fast path.
    pub(crate) async fn startup_boot(
        &self,
        object: &DynamicObject,
    ) -> Option<(String, Option<PodBinding>)> {
        let page = self
            .pods
            .list(
                &ListParams::default()
                    .labels(&pods::selector(&object.name_any()))
                    .limit(3),
            )
            .await
            .ok()?;
        let pod = self.owned_pod_from_page(object, page).ok()?;
        let boot = pod.uid().filter(|uid| !uid.is_empty())?;
        let binding = (object.metadata.deletion_timestamp.is_none() && model::ready(object))
            .then(|| model::pod_binding(object, &pod).ok())
            .flatten();
        Some((boot, binding))
    }
}

fn startup_history(pod: &Pod, expected: &PodBinding) -> Option<StartupHistory> {
    if pod.metadata.deletion_timestamp.is_some() {
        return None;
    }
    let status = pod.status.as_ref()?;
    if status.phase.as_deref() != Some("Running")
        || !status.conditions.as_ref().is_some_and(|conditions| {
            conditions
                .iter()
                .any(|condition| condition.type_ == "Ready" && condition.status == "True")
        })
    {
        return None;
    }
    let container = status
        .container_statuses
        .as_ref()?
        .iter()
        .find(|container| container.name == "runtime")?;
    let current = container.state.as_ref()?.running.as_ref()?;
    let current_process = container.container_id.as_deref()?;
    let previous = container.last_state.as_ref()?.terminated.as_ref()?;
    if !container.ready
        || Some(container.restart_count) != expected.runtime_restart_count
        || Some(current_process) != expected.runtime_container_id.as_deref()
        || previous
            .container_id
            .as_deref()
            .is_none_or(|id| id.is_empty() || id == current_process)
        || previous.exit_code == 0
    {
        return None;
    }
    let started: std::time::SystemTime = previous.started_at.as_ref()?.0.into();
    let finished: std::time::SystemTime = previous.finished_at.as_ref()?.0.into();
    let current_started: std::time::SystemTime = current.started_at.as_ref()?.0.into();
    if finished > current_started || current_started > std::time::SystemTime::now() {
        return None;
    }
    let duration = finished.duration_since(started).ok()?;
    Some(StartupHistory {
        class: termination(pod)?.class,
        duration,
    })
}

fn termination(pod: &Pod) -> Option<StartupTermination> {
    let status = pod
        .status
        .as_ref()?
        .container_statuses
        .as_ref()?
        .iter()
        .find(|status| status.name == "runtime")?;
    let terminated = status.last_state.as_ref()?.terminated.as_ref()?;
    let oom = terminated.reason.as_deref() == Some("OOMKilled");
    Some(StartupTermination {
        process: terminated.container_id.clone()?,
        failed: terminated.exit_code != 0 || oom,
        class: if oom {
            RuntimeStartupFailureClass::OutOfMemory
        } else if terminated.signal.is_some_and(|signal| signal != 0) {
            RuntimeStartupFailureClass::Terminated
        } else {
            RuntimeStartupFailureClass::ExitedBeforeServing
        },
        finished_at: terminated.finished_at.as_ref()?.0.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn startup_termination_keeps_raw_container_identity_and_only_fixed_classes() {
        for (reason, signal, exit, class, failed) in [
            (
                "Error",
                0,
                2,
                RuntimeStartupFailureClass::ExitedBeforeServing,
                true,
            ),
            (
                "OOMKilled",
                0,
                0,
                RuntimeStartupFailureClass::OutOfMemory,
                true,
            ),
            (
                "Error",
                9,
                137,
                RuntimeStartupFailureClass::Terminated,
                true,
            ),
            (
                "Completed",
                0,
                0,
                RuntimeStartupFailureClass::ExitedBeforeServing,
                false,
            ),
        ] {
            let id = format!("containerd://{}", "a".repeat(64));
            let pod: Pod = serde_json::from_value(json!({"status":{"containerStatuses":[{
                "name":"runtime","image":"runtime","imageID":"image","ready":false,"restartCount":7,
                "containerID":id,"state":{"waiting":{"reason":"CrashLoopBackOff"}},
                "lastState":{"terminated":{"containerID":id,"exitCode":exit,"reason":reason,"signal":signal,
                    "startedAt":"2026-11-17T00:00:00Z","finishedAt":"2026-11-17T00:00:02Z"}}
            }]}})).unwrap();
            let evidence = termination(&pod).unwrap();
            assert_eq!(evidence.process, id);
            assert_eq!(evidence.class, class);
            assert_eq!(evidence.failed, failed);
        }
        assert!(termination(&Pod::default()).is_none());
    }
}
