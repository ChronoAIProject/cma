use std::collections::BTreeMap;

use k8s_openapi::api::core::v1::PersistentVolumeClaim;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::{AdapterError, HomeVolumePolicy, HomeVolumeReceipt, HomeVolumeRequest, Result};

const REQUEST_ANNOTATION: &str = "cma/home-allocation";
const DIGEST_ANNOTATION: &str = "cma/home-request-sha256";

pub(super) fn recorded_request(claim: &PersistentVolumeClaim) -> Result<HomeVolumeRequest> {
    let encoded = claim
        .metadata
        .annotations
        .as_ref()
        .and_then(|annotations| annotations.get(REQUEST_ANNOTATION))
        .ok_or_else(invalid)?;
    serde_json::from_str(encoded).map_err(|_| invalid())
}

pub(super) fn claim_name(request: &HomeVolumeRequest) -> String {
    format!(
        "cma-home-{}",
        &format!("{:x}", Sha256::digest(request.volume_id.as_bytes()))[..52]
    )
}

pub(super) fn validate_scope(namespace: &str, controller: &str) -> Result<()> {
    if !dns_name(namespace, 63)
        || controller.is_empty()
        || controller.len() > 63
        || !controller
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        || !controller
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        || !controller
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
    {
        return Err(invalid());
    }
    Ok(())
}

pub(super) fn validate_policy(policy: &HomeVolumePolicy) -> Result<()> {
    if policy.is_native_vct()
        || !dns_name(&policy.storage_class, 253)
        || !dns_name(&policy.template_name, 253)
    {
        return Err(invalid());
    }
    Ok(())
}

pub(super) fn desired_claim(
    namespace: &str,
    controller: &str,
    request: &HomeVolumeRequest,
) -> Result<PersistentVolumeClaim> {
    request.validate()?;
    validate_scope(namespace, controller)?;
    validate_policy(&request.policy)?;
    let annotations = BTreeMap::from([
        (
            REQUEST_ANNOTATION,
            serde_json::to_string(request).map_err(|_| invalid())?,
        ),
        (DIGEST_ANNOTATION, request.computed_sha256()),
    ]);
    serde_json::from_value(json!({
        "apiVersion":"v1", "kind":"PersistentVolumeClaim",
        "metadata":{
            "name":claim_name(request), "namespace":namespace,
            "labels":{"cma/home-managed-by":controller},
            "annotations":annotations
        },
        "spec":{
            "accessModes":["ReadWriteOncePod"],
            "volumeMode":"Filesystem",
            "storageClassName":request.policy.storage_class,
            "resources":{"requests":{"storage":format!("{}Gi",request.policy.capacity_gib)}},
            "dataSource":{"apiGroup":"snapshot.storage.k8s.io","kind":"VolumeSnapshot","name":request.policy.template_name}
        }
    })).map_err(|_| invalid())
}

pub(super) fn validate_claim(
    namespace: &str,
    controller: &str,
    request: &HomeVolumeRequest,
    claim: &PersistentVolumeClaim,
) -> Result<HomeVolumeReceipt> {
    let mut desired = desired_claim(namespace, controller, request)?;
    if let Some(encoded) = claim
        .metadata
        .annotations
        .as_ref()
        .and_then(|values| values.get(crate::agent_sandbox::NATIVE_HOME_ANNOTATION))
    {
        let origin: crate::agent_sandbox::NativePoolHomeOrigin =
            serde_json::from_str(encoded).map_err(|_| invalid())?;
        origin.validate_for(namespace, controller, request)?;
        if claim.metadata.uid.as_deref() != Some(&origin.pvc_uid)
            || claim
                .metadata
                .labels
                .as_ref()
                .and_then(|labels| labels.get(crate::agent_sandbox::NATIVE_HOME_REQUEST_LABEL))
                .map(String::as_str)
                != Some(&request.computed_sha256()[..52])
        {
            return Err(invalid());
        }
        desired.metadata.name = Some(origin.storage_reference()?);
    }
    validate_storage(namespace, controller, request, claim)?;
    let metadata = &claim.metadata;
    let uid = metadata
        .uid
        .as_deref()
        .filter(|uid| !uid.is_empty())
        .ok_or_else(invalid)?;
    let annotations = metadata.annotations.as_ref().ok_or_else(invalid)?;
    let stored: HomeVolumeRequest =
        serde_json::from_str(annotations.get(REQUEST_ANNOTATION).ok_or_else(invalid)?)
            .map_err(|_| invalid())?;
    if metadata.name != desired.metadata.name
        || metadata.namespace.as_deref() != Some(namespace)
        || metadata
            .owner_references
            .as_ref()
            .is_some_and(|owners| !owners.is_empty())
        || metadata
            .labels
            .as_ref()
            .and_then(|labels| labels.get("cma/home-managed-by"))
            .map(String::as_str)
            != Some(controller)
        || stored != *request
        || annotations.get(DIGEST_ANNOTATION) != Some(&request.computed_sha256())
    {
        return Err(invalid());
    }
    Ok(HomeVolumeReceipt {
        native_authority: None,
        volume_id: request.volume_id.clone(),
        request_sha256: request.computed_sha256(),
        storage_reference: desired.metadata.name.ok_or_else(invalid)?,
        storage_generation: uid.to_string(),
    })
}

pub(super) fn validate_storage(
    namespace: &str,
    controller: &str,
    request: &HomeVolumeRequest,
    claim: &PersistentVolumeClaim,
) -> Result<()> {
    let desired = desired_claim(namespace, controller, request)?;
    let expected_spec = desired.spec.as_ref().ok_or_else(invalid)?;
    let spec = claim.spec.as_ref().ok_or_else(invalid)?;
    if claim.metadata.namespace.as_deref() != Some(namespace)
        || spec.access_modes != expected_spec.access_modes
        || spec
            .volume_mode
            .as_deref()
            .is_some_and(|mode| mode != "Filesystem")
        || spec.storage_class_name != expected_spec.storage_class_name
        || spec.resources != expected_spec.resources
        || spec.data_source != expected_spec.data_source
        || spec.selector.is_some()
        || spec.volume_attributes_class_name.is_some()
        || spec.data_source_ref.as_ref().is_some_and(|source| {
            source.api_group.as_deref() != Some("snapshot.storage.k8s.io")
                || source.kind != "VolumeSnapshot"
                || source.name != request.policy.template_name
                || source
                    .namespace
                    .as_deref()
                    .is_some_and(|value| value != namespace)
        })
    {
        return Err(invalid());
    }
    Ok(())
}

fn dns_name(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
}

fn invalid() -> AdapterError {
    AdapterError::Rejected(
        "durable home identity, lineage or storage policy is inconsistent".into(),
    )
}
