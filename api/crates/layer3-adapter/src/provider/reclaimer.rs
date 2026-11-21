use std::collections::BTreeMap;
use std::time::Duration;

use k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference;
use kube::api::{Api, DeleteParams, DynamicObject, ListParams, Preconditions};
use kube::core::{ApiResource, GroupVersionKind};

use super::ProviderError;
use crate::RuntimeHandle;
#[path = "runtime_finality.rs"]
pub(super) mod finality;

const SANDBOX_ID_LABEL: &str = "opensandbox.io/id";
const CREATOR_LABEL: &str = "cma/control-plane";
const GROUP: &str = "sandbox.opensandbox.io";
const VERSION: &str = "v1alpha1";
const KIND: &str = "BatchSandbox";
const PLURAL: &str = "batchsandboxes";
const DELETE_CONFIRM_TIMEOUT: Duration = Duration::from_secs(30);
const DELETE_CONFIRM_POLL: Duration = Duration::from_millis(250);
const DELETE_ATTEMPTS: usize = 3;

/// Namespaced, preconditioned access to provider-owned sandbox resources.
#[derive(Clone)]
pub struct BatchSandboxReclaimer {
    api: Api<DynamicObject>,
    pods: Api<k8s_openapi::api::core::v1::Pod>,
    scope: Option<crate::HomeVolumeScope>,
    namespace: String,
    expected_creator: Option<String>,
}

#[derive(Clone)]
pub(crate) struct CapturedSandbox {
    pub(crate) handle: RuntimeHandle,
    resource_version: String,
    identity: DeletionIdentity,
}

#[derive(Clone, PartialEq, serde::Serialize)]
struct DeletionIdentity {
    uid: String,
    generation: i64,
    labels: BTreeMap<String, String>,
    owner_references: Vec<OwnerReference>,
    spec: serde_json::Value,
}

impl BatchSandboxReclaimer {
    pub async fn infer() -> Result<Self, ProviderError> {
        let config = kube::Config::infer().await.map_err(|error| {
            ProviderError::Unreachable(format!(
                "infer Kubernetes configuration for fenced sandbox cleanup: {error}"
            ))
        })?;
        let namespace = config.default_namespace.clone();
        let authority = config.cluster_url.to_string();
        let client = kube::Client::try_from(config).map_err(|error| {
            ProviderError::Unreachable(format!(
                "build Kubernetes client for fenced sandbox cleanup: {error}"
            ))
        })?;
        Self::namespaced(client, namespace).with_finality_authority(&authority)
    }

    pub async fn infer_namespaced(namespace: impl Into<String>) -> Result<Self, ProviderError> {
        let namespace = namespace.into();
        let config = kube::Config::infer().await.map_err(|error| {
            ProviderError::Unreachable(format!(
                "infer Kubernetes configuration for fenced sandbox cleanup: {error}"
            ))
        })?;
        let authority = config.cluster_url.to_string();
        let client = kube::Client::try_from(config).map_err(|error| {
            ProviderError::Unreachable(format!(
                "build Kubernetes client for fenced sandbox cleanup: {error}"
            ))
        })?;
        Self::namespaced(client, namespace).with_finality_authority(&authority)
    }

    pub fn namespaced(client: kube::Client, namespace: impl Into<String>) -> Self {
        let namespace = namespace.into();
        let resource =
            ApiResource::from_gvk_with_plural(&GroupVersionKind::gvk(GROUP, VERSION, KIND), PLURAL);
        Self {
            api: Api::namespaced_with(client.clone(), &namespace, &resource),
            pods: Api::namespaced(client, &namespace),
            scope: None,
            namespace,
            expected_creator: None,
        }
    }

    #[must_use]
    pub fn for_creator(mut self, creator: impl Into<String>) -> Self {
        self.expected_creator = Some(creator.into());
        self
    }

    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// Prove the namespaced resource API is readable before accepting traffic.
    pub async fn preflight_cleanup_authority(&self) -> Result<(), ProviderError> {
        self.api
            .list(&ListParams::default().limit(1))
            .await
            .map(|_| ())
            .map_err(|error| kube_error("list one BatchSandbox during preflight", error))
    }

    pub(crate) async fn capture(
        &self,
        handle: &RuntimeHandle,
    ) -> Result<Option<CapturedSandbox>, ProviderError> {
        let Some(object) = self
            .api
            .get_opt(handle.as_str())
            .await
            .map_err(|error| kube_error("read BatchSandbox before fenced deletion", error))?
        else {
            return Ok(None);
        };
        self.capture_object(object, handle).map(Some)
    }

    fn capture_object(
        &self,
        object: DynamicObject,
        handle: &RuntimeHandle,
    ) -> Result<CapturedSandbox, ProviderError> {
        self.verify(&object, handle)?;
        let uid = required(object.metadata.uid.as_deref(), handle, "Kubernetes UID")?;
        let resource_version = required(
            object.metadata.resource_version.as_deref(),
            handle,
            "Kubernetes resourceVersion",
        )?;
        let generation = object
            .metadata
            .generation
            .filter(|generation| *generation > 0)
            .ok_or_else(|| {
                ProviderError::Malformed(format!(
                    "BatchSandbox {} has no positive Kubernetes generation",
                    handle.0
                ))
            })?;
        let spec = object
            .data
            .get("spec")
            .filter(|spec| spec.is_object())
            .cloned()
            .ok_or_else(|| {
                ProviderError::Malformed(format!("BatchSandbox {} has no specification", handle.0))
            })?;
        Ok(CapturedSandbox {
            handle: handle.clone(),
            resource_version,
            identity: DeletionIdentity {
                uid,
                generation,
                labels: object.metadata.labels.unwrap_or_default(),
                owner_references: object.metadata.owner_references.unwrap_or_default(),
                spec,
            },
        })
    }

    pub(crate) async fn delete(&self, mut captured: CapturedSandbox) -> Result<(), ProviderError> {
        for attempt in 0..DELETE_ATTEMPTS {
            let params = DeleteParams {
                grace_period_seconds: Some(0),
                propagation_policy: Some(kube::api::PropagationPolicy::Foreground),
                preconditions: Some(Preconditions {
                    uid: Some(captured.identity.uid.clone()),
                    resource_version: Some(captured.resource_version.clone()),
                }),
                ..DeleteParams::default()
            };
            match self.api.delete(captured.handle.as_str(), &params).await {
                Ok(_) => break,
                Err(kube::Error::Api(response)) if response.is_not_found() => return Ok(()),
                Err(kube::Error::Api(response))
                    if response.code == 409 && attempt + 1 < DELETE_ATTEMPTS =>
                {
                    // Retirement may race a status update, but must never redirect deletion.
                    let Some(current) = self.capture(&captured.handle).await? else {
                        return Ok(());
                    };
                    if current.identity != captured.identity {
                        return Err(identity_mismatch(
                            &captured.handle,
                            "identity, generation, ownership or specification changed during fenced deletion",
                        ));
                    }
                    captured.resource_version = current.resource_version;
                }
                Err(error) => {
                    return Err(kube_error(
                        "delete BatchSandbox with identity preconditions",
                        error,
                    ));
                }
            }
        }

        tokio::time::timeout(DELETE_CONFIRM_TIMEOUT, async {
            loop {
                let current = self
                    .api
                    .get_opt(captured.handle.as_str())
                    .await
                    .map_err(|error| kube_error("confirm fenced BatchSandbox deletion", error))?;
                match current {
                    None => return Ok(()),
                    Some(object) => {
                        let uid = required(
                            object.metadata.uid.as_deref(),
                            &captured.handle,
                            "Kubernetes UID during deletion confirmation",
                        )?;
                        if uid != captured.identity.uid {
                            return Ok(());
                        }
                        tokio::time::sleep(DELETE_CONFIRM_POLL).await;
                    }
                }
            }
        })
        .await
        .map_err(|_| {
            ProviderError::Unreachable(format!(
                "BatchSandbox {} was not confirmed deleted within {} seconds",
                captured.handle.0,
                DELETE_CONFIRM_TIMEOUT.as_secs()
            ))
        })?
    }

    fn verify(&self, object: &DynamicObject, handle: &RuntimeHandle) -> Result<(), ProviderError> {
        if object.metadata.name.as_deref() != Some(handle.as_str())
            || object.metadata.namespace.as_deref() != Some(self.namespace.as_str())
        {
            return Err(identity_mismatch(
                handle,
                "name or namespace differs from the captured provider object",
            ));
        }
        let labels = object
            .metadata
            .labels
            .as_ref()
            .ok_or_else(|| identity_mismatch(handle, "has no provider labels"))?;
        if labels.get(SANDBOX_ID_LABEL).map(String::as_str) != Some(handle.as_str()) {
            return Err(identity_mismatch(
                handle,
                "does not carry its provider id label",
            ));
        }
        if let Some(expected) = &self.expected_creator {
            if labels.get(CREATOR_LABEL) != Some(expected) {
                return Err(identity_mismatch(
                    handle,
                    "does not carry the expected creator label",
                ));
            }
        }
        Ok(())
    }
}

fn required(
    value: Option<&str>,
    handle: &RuntimeHandle,
    field: &str,
) -> Result<String, ProviderError> {
    value
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| {
            ProviderError::Malformed(format!("BatchSandbox {} has no {field}", handle.0))
        })
}

fn identity_mismatch(handle: &RuntimeHandle, detail: &str) -> ProviderError {
    ProviderError::IdentityMismatch(format!("BatchSandbox {} {detail}", handle.0))
}

fn kube_error(operation: &str, error: kube::Error) -> ProviderError {
    match error {
        kube::Error::Api(response) => ProviderError::Rejected {
            status: response.code,
            detail: format!("{operation}: {}", response.message),
        },
        other => ProviderError::Unreachable(format!("{operation}: {other}")),
    }
}
