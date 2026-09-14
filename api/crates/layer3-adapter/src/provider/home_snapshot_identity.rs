use kube::api::DynamicObject;
use serde_json::json;

use crate::{
    AdapterError, HomeSnapshotObservation, HomeSnapshotReceipt, HomeSnapshotRequest, Result,
};

const REQUEST: &str = "cma/home-snapshot-request";
const DIGEST: &str = "cma/home-snapshot-request-sha256";
pub(super) const LABEL: &str = "cma/home-snapshot-managed-by";

pub(super) fn name(request: &HomeSnapshotRequest) -> String {
    format!("cma-save-{}", &request.computed_sha256()[..52])
}

pub(super) fn recorded_request(snapshot: &DynamicObject) -> Result<HomeSnapshotRequest> {
    let encoded = snapshot
        .metadata
        .annotations
        .as_ref()
        .and_then(|annotations| annotations.get(REQUEST))
        .ok_or_else(invalid)?;
    serde_json::from_str(encoded).map_err(|_| invalid())
}

pub(super) fn desired(
    namespace: &str,
    controller: &str,
    request: &HomeSnapshotRequest,
) -> Result<DynamicObject> {
    request.validate()?;
    super::super::identity::validate_scope(namespace, controller)?;
    if !request
        .snapshot_class
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        || request.snapshot_class.starts_with('-')
        || request.snapshot_class.ends_with('-')
    {
        return Err(invalid());
    }
    serde_json::from_value(json!({
        "apiVersion":"snapshot.storage.k8s.io/v1", "kind":"VolumeSnapshot",
        "metadata":{
            "namespace":namespace, "name":name(request),
            "labels":{LABEL:controller},
            "annotations":{
                REQUEST:serde_json::to_string(request).map_err(|_| invalid())?,
                DIGEST:request.computed_sha256()
            }
        },
        "spec":{
            "volumeSnapshotClassName":request.snapshot_class,
            "source":{"persistentVolumeClaimName":request.home.receipt.storage_reference}
        }
    }))
    .map_err(|_| invalid())
}

pub(super) fn receipt(
    namespace: &str,
    controller: &str,
    request: &HomeSnapshotRequest,
    snapshot: &DynamicObject,
) -> Result<HomeSnapshotReceipt> {
    let desired = desired(namespace, controller, request)?;
    let metadata = &snapshot.metadata;
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
        || recorded_request(snapshot)? != *request
        || metadata
            .annotations
            .as_ref()
            .and_then(|values| values.get(DIGEST))
            != Some(&request.computed_sha256())
        || snapshot.data.get("spec") != desired.data.get("spec")
    {
        return Err(invalid());
    }
    let receipt = HomeSnapshotReceipt {
        capture_id: request.capture_id.clone(),
        request_sha256: request.computed_sha256(),
        snapshot_reference: name(request),
        snapshot_generation: metadata.uid.clone().ok_or_else(invalid)?,
    };
    receipt.validate_for(request)?;
    Ok(receipt)
}

pub(super) fn observe(
    receipt: HomeSnapshotReceipt,
    snapshot: &DynamicObject,
) -> Result<HomeSnapshotObservation> {
    if snapshot.metadata.deletion_timestamp.is_some() {
        return Err(AdapterError::Unavailable(
            "home snapshot release is pending".into(),
        ));
    }
    if snapshot
        .data
        .pointer("/status/error")
        .is_some_and(|error| !error.is_null())
    {
        return Err(AdapterError::Unavailable(
            "home snapshot controller reported a capture failure".into(),
        ));
    }
    let captured_at = match snapshot.data.pointer("/status/creationTime") {
        None | Some(serde_json::Value::Null) => None,
        Some(value) => {
            let stamp = value.as_str().ok_or_else(invalid)?;
            let _: k8s_openapi::apimachinery::pkg::apis::meta::v1::Time =
                serde_json::from_value(value.clone()).map_err(|_| invalid())?;
            Some(stamp.to_owned())
        }
    };
    let ready = match snapshot.data.pointer("/status/readyToUse") {
        None | Some(serde_json::Value::Null) => false,
        Some(value) => value.as_bool().ok_or_else(invalid)?,
    };
    if (ready && captured_at.is_none())
        || (captured_at.is_some()
            && snapshot
                .data
                .pointer("/status/boundVolumeSnapshotContentName")
                .and_then(serde_json::Value::as_str)
                .is_none_or(|name| name.is_empty()))
    {
        return Err(invalid());
    }
    Ok(HomeSnapshotObservation {
        receipt,
        captured_at,
        ready,
    })
}

fn invalid() -> AdapterError {
    AdapterError::Rejected("home snapshot storage identity is inconsistent".into())
}
