//! Observe this process's serving proof before it joins ownership placement.
use super::*;
use crate::{config::ManagerPod, health::HealthState, shutdown::Termination};
use kube::{
    api::{ApiResource, DynamicObject, GroupVersionKind},
    Api, Client,
};

const POD_PROOF_MAX_AGE: Duration = Duration::from_secs(3);
const POD_READ_DEADLINE: Duration = Duration::from_secs(2);
// The AM workload's container name is declared in infra/api/deployment.yaml.
const MANAGER_CONTAINER: &str = "api";

#[derive(Debug, Clone, PartialEq, Eq)]
struct ContainerIdentity {
    id: String,
    restarts: u64,
}

#[derive(Debug, PartialEq, Eq)]
enum PodState {
    Pending,
    Running(ContainerIdentity),
    Terminating,
    Replaced,
}

fn pod_state(pod: &DynamicObject, expected: &ManagerPod) -> Result<PodState, String> {
    if pod.metadata.uid.as_deref() != Some(&expected.uid)
        || pod.metadata.name.as_deref() != Some(&expected.name)
        || pod.metadata.namespace.as_deref() != Some(&expected.namespace)
    {
        return Err("Agent Manager Pod identity changed".into());
    }
    if pod.metadata.deletion_timestamp.is_some() {
        return Ok(PodState::Terminating);
    }
    // PodReady depends on our /ready response (including GKE NEG health).
    // Prove the running container and independent startup probe instead, then
    // combine this with application dependencies/workers in one predicate.
    if pod
        .data
        .pointer("/status/phase")
        .and_then(serde_json::Value::as_str)
        != Some("Running")
    {
        return Ok(PodState::Pending);
    }
    let Some(container) = pod
        .data
        .pointer("/status/containerStatuses")
        .and_then(serde_json::Value::as_array)
        .and_then(|containers| containers.iter().find(|c| c["name"] == MANAGER_CONTAINER))
    else {
        return Ok(PodState::Pending);
    };
    if container["started"] != true || !container["state"]["running"].is_object() {
        return Ok(PodState::Pending);
    }
    let Some(id) = container["containerID"]
        .as_str()
        .filter(|id| !id.is_empty())
    else {
        return Ok(PodState::Pending);
    };
    let Some(restarts) = container["restartCount"].as_u64() else {
        return Ok(PodState::Pending);
    };
    Ok(PodState::Running(ContainerIdentity {
        id: id.into(),
        restarts,
    }))
}

struct PodReader {
    api: Api<DynamicObject>,
    identity: ManagerPod,
    container: Mutex<Option<ContainerIdentity>>,
}
impl PodReader {
    fn new(identity: ManagerPod) -> Result<Self, String> {
        // The AM's own cluster uses its mounted workload identity, never the
        // remote Sandbox cluster or an operator kubeconfig/exec credential.
        let config = kube::Config::incluster()
            .map_err(|_| "own-Pod Kubernetes configuration unavailable")?;
        let client =
            Client::try_from(config).map_err(|_| "own-Pod Kubernetes client unavailable")?;
        let resource = ApiResource::from_gvk(&GroupVersionKind::gvk("", "v1", "Pod"));
        Ok(Self {
            api: Api::namespaced_with(client, &identity.namespace, &resource),
            identity,
            container: Mutex::new(None),
        })
    }
    async fn state(&self) -> Result<PodState, String> {
        let pod = tokio::time::timeout(POD_READ_DEADLINE, self.api.get(&self.identity.name))
            .await
            .map_err(|_| "own-Pod readiness read exceeded deadline")?
            .map_err(|_| "own-Pod readiness read unavailable")?;
        let state = pod_state(&pod, &self.identity)?;
        if let PodState::Running(current) = &state {
            let mut captured = self.container.lock().unwrap();
            if captured.as_ref().is_some_and(|old| old != current) {
                return Ok(PodState::Replaced);
            }
            *captured = Some(current.clone());
        }
        Ok(state)
    }
}

struct AdmissionProbe {
    reader: Option<PodReader>,
    proof: Arc<Mutex<Option<Instant>>>,
    termination: Termination,
}

impl AdmissionProbe {
    fn install(
        coordinator: &OwnershipCoordinator,
        reader: Option<PodReader>,
        health: HealthState,
        termination: Termination,
    ) -> Result<Self, String> {
        let proof = Arc::new(Mutex::new(None::<Instant>));
        let observed = proof.clone();
        let stop = termination.clone();
        coordinator
            .set_admission_readiness(Arc::new(move || {
                !stop.requested()
                    && health.ready()
                    && observed
                        .lock()
                        .unwrap()
                        .is_some_and(|at| at.elapsed() < POD_PROOF_MAX_AGE)
            }))
            .map_err(|error| error.to_string())?;
        Ok(Self {
            reader,
            proof,
            termination,
        })
    }

    async fn check(&self, coordinator: &OwnershipCoordinator) -> bool {
        let state = tokio::select! {
            biased;
            _ = self.termination.cancelled() => {
                coordinator.stop_admission();
                return false;
            },
            state = async {
                match &self.reader {
                    Some(reader) => reader.state().await,
                    None => Ok(PodState::Running(ContainerIdentity { id: "local-process".into(), restarts: 0 })),
                }
            } => state,
        };
        match state {
            Ok(PodState::Terminating) => {
                *self.proof.lock().unwrap() = None;
                coordinator.stop_admission();
                // Planned deletion starts ownership drain, not process failure.
                // Kubelet still needs the local HTTP helper until preStop ends
                // and SIGTERM reaches the serving loop.
                tracing::info!(
                    planned_deletion = true,
                    "Agent Manager observed Pod termination or container replacement; ownership admission closed"
                );
                return false;
            }
            Ok(PodState::Replaced) => {
                *self.proof.lock().unwrap() = None;
                coordinator.stop_admission();
                coordinator.fail().await;
                tracing::info!(
                    "Agent Manager observed Pod termination or container replacement; ownership admission closed"
                );
                return false;
            }
            Ok(PodState::Running(_)) => *self.proof.lock().unwrap() = Some(Instant::now()),
            Ok(PodState::Pending) => *self.proof.lock().unwrap() = None,
            Err(reason) => {
                *self.proof.lock().unwrap() = None;
                tracing::warn!(
                    reason,
                    "Agent Manager own-Pod proof unavailable; ownership admission closed"
                );
            }
        }
        if coordinator.is_starting() && coordinator.activate().await.is_ok() {
            tracing::info!(
                native_pod_proved = self.reader.is_some(),
                "Agent Manager ownership admission activated after readiness"
            );
        }
        true
    }
}

impl AgentManager {
    pub(crate) fn spawn_readiness(
        self: &Arc<Self>,
        pod: Option<ManagerPod>,
        health: HealthState,
        termination: Termination,
    ) -> Result<tokio::task::JoinHandle<()>, String> {
        let probe = AdmissionProbe::install(
            &self.coordinator,
            pod.map(PodReader::new).transpose()?,
            health,
            termination.clone(),
        )?;
        let weak = Arc::downgrade(self);
        Ok(tokio::spawn(async move {
            loop {
                let Some(manager) = weak.upgrade() else {
                    return;
                };
                if manager.coordinator.is_draining() {
                    return;
                }
                if !probe.check(&manager.coordinator).await {
                    manager.stop_admission();
                    // Use the same serialized 23-second drain as preStop and
                    // SIGTERM. Failure still trips the fatal watch and retains
                    // unconfirmed custody; success keeps HTTP alive for preStop.
                    if let Err(error) = manager.drain().await {
                        tracing::warn!(error, "Agent Manager readiness-triggered drain failed");
                    }
                    return;
                }
                drop(manager);
                tokio::select! {
                    biased;
                    _ = termination.cancelled() => {
                        if let Some(manager) = weak.upgrade() { manager.stop_admission(); }
                        return;
                    },
                    _ = tokio::time::sleep(Duration::from_secs(1)) => (),
                }
            }
        }))
    }
}

#[cfg(test)]
mod tests;
