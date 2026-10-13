//! Loopback runtime proxy backed by the remote Kubernetes port-forward API.
//!
//! The CMA control plane can reach the remote Kubernetes API server, but the
//! remote cluster's Pod CIDR is not routable from the CMA cluster. Keep the
//! runtime protocol unchanged and tunnel each local TCP connection through a
//! short-lived `pods/portforward` stream instead.

use k8s_openapi::api::core::v1::Pod;
use kube::api::Api;
use std::{collections::HashMap, net::SocketAddr};
use tokio::{io::copy_bidirectional, net::TcpListener, sync::Mutex};

use super::PodBinding;
use crate::{AdapterError, Result, RuntimeHandle};

const SUPERVISOR_PORT: u16 = 9090;

#[derive(Default)]
pub(crate) struct RuntimeProxies {
    entries: Mutex<HashMap<RuntimeHandle, Entry>>,
}

struct Entry {
    pod_uid: String,
    address: SocketAddr,
    task: tokio::task::JoinHandle<()>,
}

impl RuntimeProxies {
    pub(crate) async fn address(
        &self,
        runtime: &RuntimeHandle,
        binding: &PodBinding,
        pods: &Api<Pod>,
    ) -> Result<SocketAddr> {
        let mut entries = self.entries.lock().await;
        if let Some(entry) = entries.get(runtime) {
            if entry.pod_uid == binding.pod_uid {
                return Ok(entry.address);
            }
        }
        if let Some(entry) = entries.remove(runtime) {
            entry.task.abort();
        }
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(|_| AdapterError::Unavailable("runtime proxy listener unavailable".into()))?;
        let address = listener
            .local_addr()
            .map_err(|_| AdapterError::Unavailable("runtime proxy address unavailable".into()))?;
        let pod_name = binding.pod_name.clone();
        let pods = pods.clone();
        let task = tokio::spawn(async move {
            proxy_loop(listener, pods, pod_name).await;
        });
        entries.insert(
            runtime.clone(),
            Entry {
                pod_uid: binding.pod_uid.clone(),
                address,
                task,
            },
        );
        Ok(address)
    }
}

async fn proxy_loop(listener: TcpListener, pods: Api<Pod>, pod_name: String) {
    loop {
        let Ok((socket, _)) = listener.accept().await else {
            return;
        };
        let pods = pods.clone();
        let pod_name = pod_name.clone();
        tokio::spawn(async move {
            if let Err(error) = proxy_connection(socket, pods, &pod_name).await {
                tracing::debug!(pod = %pod_name, %error, "runtime port-forward connection ended");
            }
        });
    }
}

async fn proxy_connection(
    mut socket: tokio::net::TcpStream,
    pods: Api<Pod>,
    pod_name: &str,
) -> Result<()> {
    let mut forward = pods
        .portforward(pod_name, &[SUPERVISOR_PORT])
        .await
        .map_err(|error| {
            AdapterError::Unavailable(format!("runtime port-forward failed: {error}"))
        })?;
    let mut stream = forward
        .take_stream(SUPERVISOR_PORT)
        .ok_or_else(|| AdapterError::Unavailable("runtime port-forward stream missing".into()))?;
    let error = forward.take_error(SUPERVISOR_PORT).ok_or_else(|| {
        AdapterError::Unavailable("runtime port-forward error stream missing".into())
    })?;
    tokio::pin!(error);
    tokio::select! {
        result = copy_bidirectional(&mut socket, &mut stream) => {
            result.map_err(|error| AdapterError::Unavailable(format!("runtime port-forward I/O failed: {error}")))?;
        }
        message = &mut error => {
            let detail = message.unwrap_or_else(|| "remote port-forward closed".into());
            return Err(AdapterError::Unavailable(format!("runtime port-forward failed: {detail}")));
        }
    }
    forward.abort();
    let _ = forward.join().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kube::Client;

    #[tokio::test]
    async fn proxy_state_replaces_a_pod_incarnation() {
        let proxies = RuntimeProxies::default();
        let client =
            Client::try_from(kube::Config::new("http://127.0.0.1".parse().unwrap())).unwrap();
        let pods = Api::namespaced(client, "example-registry-01");
        let runtime = RuntimeHandle::new("agent-sandbox:runtime:uid");
        let first = PodBinding {
            sandbox_name: "runtime".into(),
            sandbox_uid: "sandbox-uid".into(),
            pod_name: "runtime".into(),
            pod_uid: "pod-uid-a".into(),
            pod_ip: "10.0.0.1".parse().unwrap(),
        };
        let address = proxies.address(&runtime, &first, &pods).await.unwrap();
        assert_eq!(
            proxies.address(&runtime, &first, &pods).await.unwrap(),
            address
        );
        let second = PodBinding {
            pod_uid: "pod-uid-b".into(),
            ..first
        };
        let replacement = proxies.address(&runtime, &second, &pods).await.unwrap();
        assert_ne!(replacement, address);
    }
}
