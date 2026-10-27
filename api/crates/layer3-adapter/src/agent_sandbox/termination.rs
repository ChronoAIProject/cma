//! Read exact owned runtime termination evidence only during loss recovery.
use super::{api_error, model, pods, unavailable, AgentSandboxClient};
use crate::{Result, RuntimeHandle, RuntimeTermination};
use k8s_openapi::api::core::v1::Pod;
use kube::{api::ListParams, ResourceExt};

impl AgentSandboxClient {
    pub async fn runtime_termination(
        &self,
        handle: &RuntimeHandle,
    ) -> Result<Option<RuntimeTermination>> {
        let Some(object) = self.get(handle).await? else {
            return Ok(None);
        };
        let name = object.name_any();
        let uid = object
            .uid()
            .ok_or_else(|| unavailable("Sandbox UID missing"))?;
        let page = self
            .pods
            .list(
                &ListParams::default()
                    .labels(&pods::selector(&name))
                    .limit(3),
            )
            .await
            .map_err(api_error)?;
        if page
            .metadata
            .continue_
            .is_some_and(|cursor| !cursor.is_empty())
        {
            return Err(unavailable("Sandbox termination inventory is incomplete"));
        }
        let mut owned = page
            .items
            .iter()
            .filter(|pod| model::owned_pod(pod, &self.config, &name, &uid));
        let Some(pod) = owned.next() else {
            return Ok(None);
        };
        if owned.next().is_some() {
            return Ok(None);
        }
        Ok(termination(pod))
    }
}

fn termination(pod: &Pod) -> Option<RuntimeTermination> {
    let container = pod
        .status
        .as_ref()?
        .container_statuses
        .as_ref()?
        .iter()
        .find(|container| container.name == "runtime")?;
    let stopped = container
        .state
        .as_ref()
        .and_then(|state| state.terminated.as_ref())
        .or_else(|| container.last_state.as_ref()?.terminated.as_ref())?;
    Some(RuntimeTermination {
        // Exit 137 can also be a manual kill or runtime loss; it is never OOM proof.
        out_of_memory: stopped.reason.as_deref() == Some("OOMKilled"),
        finished_at: stopped.finished_at.as_ref()?.0.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pod(reason: &str, name: &str, last: bool) -> Pod {
        let terminated = json!({"terminated": {"exitCode":137,"reason":reason,"finishedAt":"2026-10-16T09:00:00Z"}});
        let mut container = json!({"name":name,"image":"example","imageID":"example","ready":false,"restartCount":1});
        container[if last { "lastState" } else { "state" }] = terminated;
        serde_json::from_value(json!({"status":{"containerStatuses":[container]}})).unwrap()
    }

    #[test]
    fn only_explicit_oom_evidence_from_the_runtime_counts() {
        for last in [true, false] {
            assert!(
                termination(&pod("OOMKilled", "runtime", last))
                    .unwrap()
                    .out_of_memory
            );
            assert!(
                !termination(&pod("Error", "runtime", last))
                    .unwrap()
                    .out_of_memory
            );
            assert!(termination(&pod("OOMKilled", "sidecar", last)).is_none());
        }
        assert!(termination(&Pod::default()).is_none());
    }
}
