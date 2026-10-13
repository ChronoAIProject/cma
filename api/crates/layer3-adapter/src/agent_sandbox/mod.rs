//! Direct, ownership-confined integration with the infra-owned agent-sandbox API.
//! The controller creates Pods; CMA submits and observes only its own resources.

mod assignment;
mod lifecycle;
mod model;
mod suspension;

use crate::{AdapterError, HomeVolumeScope, Result, RuntimeHandle};
use k8s_openapi::api::core::v1::{PersistentVolumeClaim, Pod};
use kube::api::{DynamicObject, ListParams};
use kube::core::{ApiResource, GroupVersionKind};
use kube::{Api, Client, ResourceExt};
use std::collections::BTreeSet;

pub(crate) use model::{handle, parse_handle};
pub use model::{AgentSandboxConfig, PodBinding};
pub(crate) const OWNER_LABEL: &str = "cma/control-plane";
const RUNTIME_LABEL: &str = "cma/runtime-name";
const SPEC_ANNOTATION: &str = "cma/agent-sandbox-spec";

#[derive(Clone)]
pub struct AgentSandboxClient {
    sandboxes: Api<DynamicObject>,
    pods: Api<Pod>,
    claims: Api<PersistentVolumeClaim>,
    config: AgentSandboxConfig,
    scope: HomeVolumeScope,
}

impl AgentSandboxClient {
    pub async fn infer(config: AgentSandboxConfig) -> Result<Self> {
        let connection = kube::Config::infer()
            .await
            .map_err(|_| unavailable("Kubernetes configuration is unavailable"))?;
        let authority = connection.cluster_url.to_string();
        let client = Client::try_from(connection)
            .map_err(|_| unavailable("Kubernetes client could not be initialized"))?;
        Self::new(client, config, &authority)
    }

    pub fn new(client: Client, config: AgentSandboxConfig, authority: &str) -> Result<Self> {
        config.validate()?;
        let resource = ApiResource::from_gvk_with_plural(
            &GroupVersionKind::gvk("agents.x-k8s.io", "v1beta1", "Sandbox"),
            "sandboxes",
        );
        let scope = HomeVolumeScope::from_authority(
            "agent-sandbox-v1beta1",
            &[authority, &config.namespace, &config.deployment],
        )?;
        Ok(Self {
            sandboxes: Api::namespaced_with(client.clone(), &config.namespace, &resource),
            pods: Api::namespaced(client.clone(), &config.namespace),
            claims: Api::namespaced(client, &config.namespace),
            config,
            scope,
        })
    }

    fn selector(&self) -> String {
        format!("{OWNER_LABEL}={}", self.config.deployment)
    }

    fn owned(&self, object: &DynamicObject) -> Result<()> {
        if object.namespace().as_deref() != Some(&self.config.namespace)
            || object.labels().get(OWNER_LABEL) != Some(&self.config.deployment)
            || object.uid().as_deref().is_none_or(str::is_empty)
            || object
                .resource_version()
                .as_deref()
                .is_none_or(str::is_empty)
        {
            return Err(rejected("Sandbox is not owned by this CMA deployment"));
        }
        Ok(())
    }

    pub async fn get(&self, runtime: &RuntimeHandle) -> Result<Option<DynamicObject>> {
        let (name, uid) = parse_handle(runtime)?;
        let Some(object) = self.sandboxes.get_opt(name).await.map_err(api_error)? else {
            return Ok(None);
        };
        self.owned(&object)?;
        if object.uid().as_deref() != Some(uid) {
            return Err(rejected("Sandbox name now belongs to a different UID"));
        }
        Ok(Some(object))
    }

    pub async fn inventory(&self) -> Result<Vec<DynamicObject>> {
        let mut parameters = ListParams::default().labels(&self.selector()).limit(200);
        let mut result = Vec::new();
        let mut cursors = BTreeSet::new();
        loop {
            let page = self.sandboxes.list(&parameters).await.map_err(api_error)?;
            for object in page.items {
                // Also verify ownership locally: a misconfigured/faulty endpoint
                // returning foreign resources cannot make them cleanup candidates.
                if object.labels().get(OWNER_LABEL) != Some(&self.config.deployment) {
                    continue;
                }
                self.owned(&object)?;
                result.push(object);
                if result.len() > 10_000 {
                    return Err(unavailable("CMA inventory exceeds its bound"));
                }
            }
            let Some(cursor) = page.metadata.continue_.filter(|s| !s.is_empty()) else {
                return Ok(result);
            };
            if !cursors.insert(cursor.clone()) {
                return Err(unavailable("Sandbox inventory did not advance"));
            }
            parameters = parameters.continue_token(&cursor);
        }
    }

    pub async fn pod_binding(&self, runtime: &RuntimeHandle) -> Result<PodBinding> {
        let object = self
            .get(runtime)
            .await?
            .ok_or_else(|| unavailable("Sandbox no longer exists"))?;
        if object.metadata.deletion_timestamp.is_some() || !model::ready(&object) {
            return Err(unavailable("Sandbox is not ready"));
        }
        let name = object.name_any();
        let uid = object.uid().ok_or_else(|| rejected("Sandbox has no UID"))?;
        let page = self
            .pods
            .list(
                &ListParams::default()
                    .labels(&format!("{},{RUNTIME_LABEL}={name}", self.selector()))
                    .limit(3),
            )
            .await
            .map_err(api_error)?;
        if page.metadata.continue_.is_some_and(|s| !s.is_empty()) {
            return Err(unavailable("Sandbox Pod inventory is incomplete"));
        }
        let mut owned = page
            .items
            .into_iter()
            .filter(|pod| model::owned_pod(pod, &self.config, &name, &uid));
        let pod = owned
            .next()
            .ok_or_else(|| unavailable("Sandbox has no owned Pod"))?;
        if owned.next().is_some() {
            return Err(unavailable("Sandbox has multiple owned Pods"));
        }
        model::pod_binding(&object, &pod)
    }

    pub(crate) fn control_token(&self, runtime_name: &str) -> String {
        use ring::hmac;
        let key = hmac::Key::new(hmac::HMAC_SHA256, self.config.control_key.as_bytes());
        let message = format!(
            "cma-agent-sandbox-control-v1\0{}\0{}\0{runtime_name}",
            self.config.namespace, self.config.deployment
        );
        hmac::sign(&key, message.as_bytes())
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }
}

fn rejected(detail: &str) -> AdapterError {
    AdapterError::Rejected(detail.into())
}
fn unavailable(detail: &str) -> AdapterError {
    AdapterError::Unavailable(detail.into())
}
fn api_error(error: kube::Error) -> AdapterError {
    match error {
        kube::Error::Api(response) if response.code == 429 => {
            AdapterError::AtCapacity("Kubernetes API throttled the request".into())
        }
        kube::Error::Api(response) if matches!(response.code, 400 | 401 | 403 | 409 | 422) => {
            rejected(&format!(
                "Kubernetes rejected Sandbox operation ({})",
                response.code
            ))
        }
        _ => unavailable("Kubernetes Sandbox operation failed"),
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod http_tests;
