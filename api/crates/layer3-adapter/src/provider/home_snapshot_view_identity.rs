use k8s_openapi::api::core::v1::PersistentVolumeClaim;
use serde_json::json;

use crate::{
    AdapterError, HomeSnapshotExportRequest, HomeSnapshotViewObservation, HomeSnapshotViewReceipt,
    Result,
};

pub(super) const LABEL: &str = "cma/home-view-managed-by";
const REQUEST: &str = "cma/home-view-request";
const DIGEST: &str = "cma/home-view-request-sha256";

pub(super) fn name(request: &HomeSnapshotExportRequest) -> Result<String> {
    Ok(format!("cma-view-{}", &request.computed_sha256()?[..52]))
}

pub(super) fn recorded_request(claim: &PersistentVolumeClaim) -> Result<HomeSnapshotExportRequest> {
    serde_json::from_str(
        claim
            .metadata
            .annotations
            .as_ref()
            .and_then(|values| values.get(REQUEST))
            .ok_or_else(invalid)?,
    )
    .map_err(|_| invalid())
}

pub(super) fn desired(
    namespace: &str,
    controller: &str,
    request: &HomeSnapshotExportRequest,
) -> Result<PersistentVolumeClaim> {
    request.validate()?;
    super::super::identity::validate_scope(namespace, controller)?;
    super::super::identity::validate_policy(&request.snapshot.home.request.policy)?;
    let name = name(request)?;
    if name == request.snapshot.home.receipt.storage_reference {
        return Err(invalid());
    }
    serde_json::from_value(json!({
        "apiVersion":"v1", "kind":"PersistentVolumeClaim",
        "metadata":{
            "namespace":namespace, "name":name,
            "labels":{LABEL:controller},
            "annotations":{
                REQUEST:serde_json::to_string(request).map_err(|_| invalid())?,
                DIGEST:request.computed_sha256()?
            }
        },
        "spec":{
            "accessModes":["ReadWriteOncePod"], "volumeMode":"Filesystem",
            "storageClassName":request.snapshot.home.request.policy.storage_class,
            "resources":{"requests":{"storage":format!("{}Gi",request.snapshot.home.request.policy.capacity_gib)}},
            "dataSource":{
                "apiGroup":"snapshot.storage.k8s.io", "kind":"VolumeSnapshot",
                "name":request.observation.receipt.snapshot_reference
            }
        }
    })).map_err(|_| invalid())
}

pub(super) fn receipt(
    namespace: &str,
    controller: &str,
    request: &HomeSnapshotExportRequest,
    claim: &PersistentVolumeClaim,
) -> Result<HomeSnapshotViewReceipt> {
    let desired = desired(namespace, controller, request)?;
    let expected = desired.spec.as_ref().ok_or_else(invalid)?;
    let spec = claim.spec.as_ref().ok_or_else(invalid)?;
    let metadata = &claim.metadata;
    if metadata.name != desired.metadata.name
        || metadata.namespace != desired.metadata.namespace
        || metadata
            .owner_references
            .as_ref()
            .is_some_and(|owners| !owners.is_empty())
        || metadata
            .labels
            .as_ref()
            .and_then(|labels| labels.get(LABEL))
            .map(String::as_str)
            != Some(controller)
        || recorded_request(claim)? != *request
        || metadata
            .annotations
            .as_ref()
            .and_then(|values| values.get(DIGEST))
            != Some(&request.computed_sha256()?)
        || spec.access_modes != expected.access_modes
        || spec
            .volume_mode
            .as_deref()
            .is_some_and(|mode| mode != "Filesystem")
        || spec.storage_class_name != expected.storage_class_name
        || spec.resources != expected.resources
        || spec.data_source != expected.data_source
        || spec.selector.is_some()
        || spec.volume_attributes_class_name.is_some()
        || spec.data_source_ref.as_ref().is_some_and(|source| {
            source.api_group.as_deref() != Some("snapshot.storage.k8s.io")
                || source.kind != "VolumeSnapshot"
                || source.name != request.observation.receipt.snapshot_reference
                || source
                    .namespace
                    .as_deref()
                    .is_some_and(|value| value != namespace)
        })
    {
        return Err(invalid());
    }
    let receipt = HomeSnapshotViewReceipt {
        request_sha256: request.computed_sha256()?,
        volume_reference: name(request)?,
        volume_generation: metadata.uid.clone().ok_or_else(invalid)?,
    };
    receipt.validate_for(request)?;
    Ok(receipt)
}

pub(super) fn observe(
    receipt: HomeSnapshotViewReceipt,
    claim: &PersistentVolumeClaim,
) -> Result<HomeSnapshotViewObservation> {
    if claim.metadata.deletion_timestamp.is_some() {
        return Err(AdapterError::Unavailable(
            "snapshot export view release is pending".into(),
        ));
    }
    let phase = claim
        .status
        .as_ref()
        .and_then(|status| status.phase.as_deref());
    let bound_volume = match phase {
        None | Some("Pending") => None,
        Some("Bound") => Some(
            claim
                .spec
                .as_ref()
                .and_then(|spec| spec.volume_name.clone())
                .filter(|name| {
                    !name.is_empty() && name.trim() == name && !name.chars().any(char::is_control)
                })
                .ok_or_else(invalid)?,
        ),
        _ => return Err(invalid()),
    };
    Ok(HomeSnapshotViewObservation {
        receipt,
        bound_volume,
    })
}

pub(super) fn invalid() -> AdapterError {
    AdapterError::Rejected("snapshot export view storage identity is inconsistent".into())
}
