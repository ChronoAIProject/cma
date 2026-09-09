use k8s_openapi::api::core::v1::PersistentVolumeClaim;
use kube::api::{Api, DeleteParams, DynamicObject, ListParams, PostParams, Preconditions};
use kube::core::{ApiResource, GroupVersionKind};

use crate::{
    AdapterError, HomeVolumeAttachment, HomeVolumePolicy, HomeVolumeReceipt, HomeVolumeRequest,
    HomeVolumeStore, Result,
};

#[path = "home_volume_identity.rs"]
mod identity;

#[path = "home_snapshots.rs"]
mod snapshots;

/// Namespaced disk allocation and cleanup beneath recorded Sandbox home authority.
#[derive(Clone)]
pub struct PersistentHomeVolumes {
    claims: Api<PersistentVolumeClaim>,
    templates: Api<DynamicObject>,
    namespace: String,
    controller: String,
}

impl PersistentHomeVolumes {
    pub async fn infer_namespaced(
        namespace: impl Into<String>,
        controller: impl Into<String>,
    ) -> Result<Self> {
        let client = kube::Client::try_default().await.map_err(|_| {
            AdapterError::Unavailable("durable home Kubernetes authority is unavailable".into())
        })?;
        Self::namespaced(client, namespace, controller)
    }

    pub fn namespaced(
        client: kube::Client,
        namespace: impl Into<String>,
        controller: impl Into<String>,
    ) -> Result<Self> {
        let namespace = namespace.into();
        let controller = controller.into();
        identity::validate_scope(&namespace, &controller)?;
        let resource = ApiResource::from_gvk_with_plural(
            &GroupVersionKind::gvk("snapshot.storage.k8s.io", "v1", "VolumeSnapshot"),
            "volumesnapshots",
        );
        Ok(Self {
            claims: Api::namespaced(client.clone(), &namespace),
            templates: Api::namespaced_with(client, &namespace, &resource),
            namespace,
            controller,
        })
    }

    pub async fn preflight(&self, policy: &HomeVolumePolicy) -> Result<()> {
        self.verify_template(policy).await?;
        self.claims
            .list(&ListParams::default().limit(1))
            .await
            .map_err(|error| failure("read home volume authority", error))?;
        Ok(())
    }

    async fn verify_template(&self, policy: &HomeVolumePolicy) -> Result<()> {
        policy.validate()?;
        identity::validate_policy(policy)?;
        let template = self
            .templates
            .get(&policy.template_name)
            .await
            .map_err(|error| failure("read initialized home template", error))?;
        if template.metadata.uid.as_deref() != Some(policy.template_generation.as_str())
            || template.metadata.deletion_timestamp.is_some()
            || template
                .metadata
                .annotations
                .as_ref()
                .and_then(|values| values.get("cma/home-seed-format"))
                .map(String::as_str)
                != Some("1")
            || template
                .data
                .pointer("/status/readyToUse")
                .and_then(serde_json::Value::as_bool)
                != Some(true)
        {
            return Err(AdapterError::Rejected(
                "initialized home template is unavailable or changed identity".into(),
            ));
        }
        Ok(())
    }

    async fn read(&self, request: &HomeVolumeRequest) -> Result<Option<PersistentVolumeClaim>> {
        self.claims
            .get_opt(&identity::claim_name(request))
            .await
            .map_err(|error| failure("read durable home", error))
    }

    fn receipt(
        &self,
        request: &HomeVolumeRequest,
        claim: &PersistentVolumeClaim,
    ) -> Result<HomeVolumeReceipt> {
        identity::validate_claim(&self.namespace, &self.controller, request, claim)
    }
}

#[async_trait::async_trait]
impl HomeVolumeStore for PersistentHomeVolumes {
    async fn inventory(&self) -> Result<Vec<HomeVolumeAttachment>> {
        let mut params = ListParams::default()
            .labels(&format!("cma/home-managed-by={}", self.controller))
            .limit(200);
        let mut cursors = std::collections::BTreeSet::new();
        let mut volumes = std::collections::BTreeSet::new();
        let mut inventory = Vec::new();
        loop {
            let page = self
                .claims
                .list(&params)
                .await
                .map_err(|error| failure("list durable home authority", error))?;
            for claim in page.items {
                let request = identity::recorded_request(&claim)?;
                let receipt = self.receipt(&request, &claim)?;
                receipt.validate_for(&request)?;
                if !volumes.insert(request.volume_id.clone()) {
                    return Err(AdapterError::Rejected(
                        "durable home inventory repeats an identity".into(),
                    ));
                }
                inventory.push(HomeVolumeAttachment { request, receipt });
            }
            let Some(cursor) = page.metadata.continue_.filter(|value| !value.is_empty()) else {
                return Ok(inventory);
            };
            if !cursors.insert(cursor.clone()) {
                return Err(AdapterError::Rejected(
                    "durable home inventory did not advance".into(),
                ));
            }
            params = params.continue_token(&cursor);
        }
    }

    async fn prepare(&self, request: &HomeVolumeRequest) -> Result<HomeVolumeReceipt> {
        request.validate()?;
        self.verify_template(&request.policy).await?;
        let claim = match self.read(request).await? {
            Some(claim) => claim,
            None => {
                let desired = identity::desired_claim(&self.namespace, &self.controller, request)?;
                match self.claims.create(&PostParams::default(), &desired).await {
                    Ok(claim) => claim,
                    Err(error) => {
                        // Creation can commit before its response is lost. A matching
                        // physical row resolves that ambiguity without allocating again.
                        match self.read(request).await? {
                            Some(claim) => claim,
                            None => return Err(failure("allocate durable home", error)),
                        }
                    }
                }
            }
        };
        if claim.metadata.deletion_timestamp.is_some() {
            return Err(AdapterError::Unavailable(
                "durable home is being released".into(),
            ));
        }
        self.receipt(request, &claim)
    }

    async fn verify(&self, attachment: &HomeVolumeAttachment) -> Result<()> {
        attachment.receipt.validate_for(&attachment.request)?;
        self.verify_template(&attachment.request.policy).await?;
        let claim = self.read(&attachment.request).await?.ok_or_else(|| {
            AdapterError::Rejected("the recorded durable home no longer exists".into())
        })?;
        if claim.metadata.deletion_timestamp.is_some()
            || self.receipt(&attachment.request, &claim)? != attachment.receipt
        {
            return Err(AdapterError::Rejected(
                "the recorded durable home changed identity".into(),
            ));
        }
        Ok(())
    }

    async fn remove(
        &self,
        request: &HomeVolumeRequest,
        receipt: Option<&HomeVolumeReceipt>,
    ) -> Result<()> {
        request.validate()?;
        if let Some(receipt) = receipt {
            receipt.validate_for(request)?;
        }
        let Some(claim) = self.read(request).await? else {
            return Ok(());
        };
        let captured = self.receipt(request, &claim)?;
        if receipt.is_some_and(|expected| expected != &captured) {
            return Err(AdapterError::Rejected(
                "durable home cleanup target was replaced".into(),
            ));
        }
        let version = claim
            .metadata
            .resource_version
            .filter(|value| !value.is_empty())
            .ok_or_else(|| AdapterError::Rejected("durable home has no resource version".into()))?;
        if claim.metadata.deletion_timestamp.is_none() {
            let params = DeleteParams {
                preconditions: Some(Preconditions {
                    uid: Some(captured.storage_generation.clone()),
                    resource_version: Some(version),
                }),
                ..DeleteParams::default()
            };
            match self
                .claims
                .delete(&captured.storage_reference, &params)
                .await
            {
                Ok(_) => (),
                Err(kube::Error::Api(response)) if response.is_not_found() => return Ok(()),
                Err(error) => return Err(failure("release exact durable home", error)),
            }
        }
        match self.read(request).await? {
            None => Ok(()),
            Some(current)
                if current.metadata.uid.as_deref() != Some(&captured.storage_generation)
                    && current
                        .metadata
                        .uid
                        .as_ref()
                        .is_some_and(|uid| !uid.is_empty()) =>
            {
                Ok(())
            }
            Some(_) => Err(AdapterError::Unavailable(
                "durable home release is still pending".into(),
            )),
        }
    }
}

fn failure(operation: &str, error: kube::Error) -> AdapterError {
    match error {
        kube::Error::Api(response) if matches!(response.code, 400 | 401 | 403 | 422) => {
            AdapterError::Rejected(format!("{operation}: Kubernetes status {}", response.code))
        }
        _ => AdapterError::Unavailable(format!("{operation}: Kubernetes request did not complete")),
    }
}

#[cfg(test)]
#[path = "home_volumes_tests.rs"]
mod tests;
