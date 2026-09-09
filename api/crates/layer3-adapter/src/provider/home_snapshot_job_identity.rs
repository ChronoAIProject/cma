use k8s_openapi::api::core::v1::Pod;

use crate::{
    HomeSnapshotExportJobPhase as Phase, HomeSnapshotExportJobReceipt,
    HomeSnapshotExportJobRequest, Result,
};

use super::spec::{self, invalid};

pub(super) fn recorded_request(pod: &Pod) -> Result<HomeSnapshotExportJobRequest> {
    serde_json::from_str(
        pod.metadata
            .annotations
            .as_ref()
            .and_then(|values| values.get(spec::REQUEST))
            .ok_or_else(invalid)?,
    )
    .map_err(|_| invalid())
}

pub(super) fn receipt(
    namespace: &str,
    controller: &str,
    request: &HomeSnapshotExportJobRequest,
    pod: &Pod,
) -> Result<HomeSnapshotExportJobReceipt> {
    let desired = spec::desired(namespace, controller, request)?;
    let expected = desired.spec.as_ref().ok_or_else(invalid)?;
    let actual = pod.spec.as_ref().ok_or_else(invalid)?;
    let annotations = pod.metadata.annotations.as_ref().ok_or_else(invalid)?;
    if pod.metadata.name != desired.metadata.name
        || pod.metadata.namespace != desired.metadata.namespace
        || pod
            .metadata
            .owner_references
            .as_ref()
            .is_some_and(|owners| !owners.is_empty())
        || pod
            .metadata
            .labels
            .as_ref()
            .and_then(|values| values.get(spec::LABEL))
            .map(String::as_str)
            != Some(controller)
        || pod
            .metadata
            .labels
            .as_ref()
            .and_then(|values| values.get("app.kubernetes.io/name"))
            .map(String::as_str)
            != Some("cma-snapshot-export")
        || recorded_request(pod)? != *request
        || [spec::DIGEST, spec::INPUT].iter().any(|key| {
            annotations.get(*key)
                != desired
                    .metadata
                    .annotations
                    .as_ref()
                    .and_then(|values| values.get(*key))
        })
        || actual.containers != expected.containers
        || actual.init_containers != expected.init_containers
        || actual.volumes != expected.volumes
        || actual.restart_policy != expected.restart_policy
        || actual.active_deadline_seconds != expected.active_deadline_seconds
        || actual.termination_grace_period_seconds != expected.termination_grace_period_seconds
        || actual.service_account_name != expected.service_account_name
        || actual.automount_service_account_token != Some(false)
        || actual.enable_service_links != Some(false)
        || actual.security_context != expected.security_context
        || actual.runtime_class_name != expected.runtime_class_name
        || actual.node_selector != expected.node_selector
        || actual.host_network.unwrap_or(false)
        || actual.host_pid.unwrap_or(false)
        || actual.host_ipc.unwrap_or(false)
        || actual.share_process_namespace.unwrap_or(false)
        || actual
            .ephemeral_containers
            .as_ref()
            .is_some_and(|values| !values.is_empty())
        || actual
            .image_pull_secrets
            .as_ref()
            .is_some_and(|values| !values.is_empty())
    {
        return Err(invalid());
    }
    let receipt = HomeSnapshotExportJobReceipt {
        request_sha256: request.computed_sha256()?,
        pod_reference: spec::name(request)?,
        pod_generation: pod.metadata.uid.clone().ok_or_else(invalid)?,
    };
    receipt.validate_for(request)?;
    Ok(receipt)
}

pub(super) fn phase(pod: &Pod) -> Result<Phase> {
    if pod.metadata.deletion_timestamp.is_some() {
        return Err(crate::AdapterError::Unavailable(
            "snapshot exporter removal is pending".into(),
        ));
    }
    let Some(status) = &pod.status else {
        return Ok(Phase::Pending);
    };
    if matches!(
        status.phase.as_deref(),
        Some("Failed" | "Succeeded" | "Unknown")
    ) {
        return Ok(Phase::Failed);
    }
    let initial = status
        .init_container_statuses
        .as_deref()
        .unwrap_or_default();
    let transfer = status.container_statuses.as_deref().unwrap_or_default();
    if initial
        .iter()
        .any(|item| item.name != "export" || item.restart_count != 0)
        || transfer
            .iter()
            .any(|item| item.name != "transfer" || item.restart_count != 0)
        || initial.len() > 1
        || transfer.len() > 1
    {
        return Err(invalid());
    }
    let initial_state = initial.first().and_then(|item| item.state.as_ref());
    if initial_state
        .and_then(|state| state.terminated.as_ref())
        .is_some_and(|state| state.exit_code != 0)
        || transfer
            .first()
            .and_then(|item| item.state.as_ref())
            .is_some_and(|state| state.terminated.is_some())
    {
        return Ok(Phase::Failed);
    }
    if initial_state
        .and_then(|state| state.terminated.as_ref())
        .is_some_and(|state| state.exit_code == 0)
        && transfer.first().is_some_and(|item| {
            item.ready
                && item
                    .state
                    .as_ref()
                    .is_some_and(|state| state.running.is_some())
        })
    {
        return Ok(Phase::Ready);
    }
    Ok(
        if initial_state.is_some_and(|state| state.running.is_some() || state.terminated.is_some())
        {
            Phase::Exporting
        } else {
            Phase::Pending
        },
    )
}
