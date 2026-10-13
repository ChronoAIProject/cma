use k8s_openapi::api::core::v1::{PersistentVolumeClaim, Pod};
use kube::api::{Api, DeleteParams, DynamicObject, ListParams, PostParams, Preconditions};
use kube::core::{ApiResource, GroupVersionKind};

use crate::{
    AdapterError, HomeVolumeAttachment, HomeVolumePolicy, HomeVolumeReceipt, HomeVolumeRequest,
    HomeVolumeScope, HomeVolumeStore, Result,
};

#[path = "home_volume_identity.rs"]
mod identity;

#[path = "home_native_recovery.rs"]
mod native_recovery;

#[path = "home_original_claim.rs"]
mod original_claim;

#[path = "home_finality_receipts.rs"]
mod finality_receipts;

#[path = "home_snapshots.rs"]
mod snapshots;

#[path = "home_snapshot_views.rs"]
mod snapshot_views;

#[path = "home_snapshot_jobs.rs"]
mod snapshot_jobs;

/// Namespaced disk allocation and cleanup beneath recorded Sandbox home authority.
#[derive(Clone)]
pub struct PersistentHomeVolumes {
    claims: Api<PersistentVolumeClaim>,
    templates: Api<DynamicObject>,
    export_pods: Api<Pod>,
    namespace: String,
    controller: String,
    scope: HomeVolumeScope,
}

impl PersistentHomeVolumes {
    fn validate_request_scope(&self, request: &HomeVolumeRequest) -> Result<()> {
        request.validate()?;
        if request
            .scope
            .as_ref()
            .is_some_and(|scope| scope != &self.scope)
        {
            return Err(AdapterError::Rejected(
                "home allocation belongs to another Kubernetes authority".into(),
            ));
        }
        Ok(())
    }
    pub(crate) fn validate_pool_storage(
        namespace: &str,
        controller: &str,
        request: &HomeVolumeRequest,
        claim: &PersistentVolumeClaim,
    ) -> Result<()> {
        identity::validate_storage(namespace, controller, request, claim)
    }

    pub(crate) fn native_home_receipt(
        namespace: &str,
        controller: &str,
        request: &HomeVolumeRequest,
        claim: &PersistentVolumeClaim,
    ) -> Result<HomeVolumeReceipt> {
        identity::validate_claim(namespace, controller, request, claim)
    }
    pub async fn infer_namespaced(
        namespace: impl Into<String>,
        controller: impl Into<String>,
    ) -> Result<Self> {
        let config = kube::Config::infer().await.map_err(|_| {
            AdapterError::Unavailable("durable home Kubernetes authority is unavailable".into())
        })?;
        let authority = config.cluster_url.to_string();
        let client = kube::Client::try_from(config).map_err(|_| {
            AdapterError::Unavailable("durable home Kubernetes authority is unavailable".into())
        })?;
        Self::namespaced(client, namespace, controller, &authority)
    }

    pub fn namespaced(
        client: kube::Client,
        namespace: impl Into<String>,
        controller: impl Into<String>,
        api_authority: &str,
    ) -> Result<Self> {
        let namespace = namespace.into();
        let controller = controller.into();
        identity::validate_scope(&namespace, &controller)?;
        let scope = HomeVolumeScope::from_authority(
            "kubernetes-pvc-v1",
            &[api_authority, &namespace, &controller],
        )?;
        let resource = ApiResource::from_gvk_with_plural(
            &GroupVersionKind::gvk("snapshot.storage.k8s.io", "v1", "VolumeSnapshot"),
            "volumesnapshots",
        );
        Ok(Self {
            claims: Api::namespaced(client.clone(), &namespace),
            export_pods: Api::namespaced(client.clone(), &namespace),
            templates: Api::namespaced_with(client, &namespace, &resource),
            namespace,
            controller,
            scope,
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
        self.validate_request_scope(request)?;
        self.claims
            .get_opt(&identity::claim_name(request))
            .await
            .map_err(|error| failure("read durable home", error))
    }

    async fn read_receipt(
        &self,
        request: &HomeVolumeRequest,
        receipt: Option<&HomeVolumeReceipt>,
    ) -> Result<Option<PersistentVolumeClaim>> {
        self.validate_request_scope(request)?;
        if let Some(receipt) = receipt {
            receipt.validate_for(request)?;
            // The receipt pins an adopted pool PVC's actual name and UID. The
            // identity validator additionally checks its original claim lineage.
            self.claims
                .get_opt(&receipt.storage_reference)
                .await
                .map_err(|error| failure("read recorded durable home", error))
        } else {
            let ordinary = self.read(request).await?;
            let adopted = Self::recover_native_claim(
                &self.claims,
                &self.namespace,
                &self.controller,
                request,
            )
            .await?;
            match (ordinary, adopted) {
                (Some(_), Some(_)) => Err(AdapterError::Rejected(
                    "durable home has conflicting ordinary and native allocations".into(),
                )),
                (ordinary, adopted) => Ok(ordinary.or(adopted)),
            }
        }
    }

    fn receipt(
        &self,
        request: &HomeVolumeRequest,
        claim: &PersistentVolumeClaim,
    ) -> Result<HomeVolumeReceipt> {
        self.validate_request_scope(request)?;
        identity::validate_claim(&self.namespace, &self.controller, request, claim)
    }
}

#[async_trait::async_trait]
impl HomeVolumeStore for PersistentHomeVolumes {
    async fn delete_original_home(
        &self,
        original: &crate::HomeProvisionReceipt,
        operation_id: &str,
    ) -> Result<crate::HomeDeleteReceipt> {
        self.delete_home_acknowledged(original, operation_id).await
    }
    async fn observe_home_finality(
        &self,
        delete: &crate::HomeDeleteReceipt,
    ) -> Result<crate::HomeFinalityReceipt> {
        self.observe_home_claim_absence(delete).await
    }
    async fn prepare_receipted(
        &self,
        request: &HomeVolumeRequest,
        operation_id: &str,
    ) -> Result<crate::HomeProvisionReceipt> {
        self.prepare_original(request, operation_id).await
    }

    fn allocation_scope(&self) -> Option<HomeVolumeScope> {
        Some(self.scope.clone())
    }
    async fn remove_captures(&self, attachment: &HomeVolumeAttachment) -> Result<()> {
        use crate::{HomeSnapshotExportJobStore, HomeSnapshotStore, HomeSnapshotViewStore};
        attachment.receipt.validate_for(&attachment.request)?;
        self.validate_request_scope(&attachment.request)?;
        for entry in self.export_job_inventory().await? {
            if entry.request.export.snapshot.home == *attachment {
                self.remove_export_job(&entry.request, Some(&entry.receipt))
                    .await?;
            }
        }
        for entry in self.view_inventory().await? {
            if entry.request.snapshot.home == *attachment {
                self.remove_view(&entry.request, Some(&entry.receipt))
                    .await?;
            }
        }
        for entry in self.snapshot_inventory().await? {
            if entry.request.home == *attachment {
                self.remove_snapshot(&entry.request, Some(&entry.receipt))
                    .await?;
            }
        }
        Ok(())
    }

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
        self.validate_request_scope(request)?;
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
        self.validate_request_scope(&attachment.request)?;
        self.verify_template(&attachment.request.policy).await?;
        let claim = self
            .read_receipt(&attachment.request, Some(&attachment.receipt))
            .await?
            .ok_or_else(|| {
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
        self.validate_request_scope(request)?;
        if let Some(receipt) = receipt {
            receipt.validate_for(request)?;
        }
        let Some(claim) = self.read_receipt(request, receipt).await? else {
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
        match self.read_receipt(request, Some(&captured)).await? {
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
