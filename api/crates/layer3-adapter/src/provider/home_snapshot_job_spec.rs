use k8s_openapi::api::core::v1::{Container, Pod};
use serde_json::json;

use crate::{AdapterError, HomeSnapshotExportJobRequest, Result};

pub(super) const LABEL: &str = "cma/home-export-managed-by";
pub(super) const REQUEST: &str = "cma/home-export-request";
pub(super) const DIGEST: &str = "cma/home-export-digest";
pub(super) const INPUT: &str = "cma/home-export-input";
pub(super) const OUTPUT: &str = "/var/lib/cma-export";

pub(super) fn name(request: &HomeSnapshotExportJobRequest) -> Result<String> {
    Ok(format!("cma-export-{}", &request.computed_sha256()?[..48]))
}

pub(super) fn desired(
    namespace: &str,
    controller: &str,
    request: &HomeSnapshotExportJobRequest,
) -> Result<Pod> {
    request.validate()?;
    super::super::identity::validate_scope(namespace, controller)?;
    let security = json!({"allowPrivilegeEscalation":false,"readOnlyRootFilesystem":true,"capabilities":{"drop":["ALL"]}});
    let export: Container = serde_json::from_value(json!({
        "name":"export", "image":request.image, "imagePullPolicy":"IfNotPresent",
        "command":["/usr/local/bin/layer4-runtime"],
        "args":["--export-home-snapshot","/var/run/cma-export/request.json",OUTPUT],
        "terminationMessagePath":"/dev/termination-log", "terminationMessagePolicy":"File",
        "securityContext":security,
        "resources":{"requests":{"cpu":"250m","memory":"256Mi","ephemeral-storage":"1Gi"},"limits":{"cpu":"2","memory":"2Gi","ephemeral-storage":"20Gi"}},
        "volumeMounts":[
            {"name":"snapshot","mountPath":"/home/cma","subPath":"home","readOnly":true},
            {"name":"snapshot","mountPath":"/var/lib/cma-runtime","subPath":"state","readOnly":true},
            {"name":"snapshot","mountPath":"/var/lib/cma-transfer","subPath":"transfer","readOnly":true},
            {"name":"input","mountPath":"/var/run/cma-export","readOnly":true},
            {"name":"output","mountPath":OUTPUT}
        ]
    })).map_err(|_| invalid())?;
    let transfer: Container = serde_json::from_value(json!({
        "name":"transfer", "image":request.image, "imagePullPolicy":"IfNotPresent",
        "command":["/bin/sleep"], "args":["infinity"],
        "terminationMessagePath":"/dev/termination-log", "terminationMessagePolicy":"File",
        "env":[{"name":"CMA_EXPORT_POD_UID","valueFrom":{"fieldRef":{"apiVersion":"v1","fieldPath":"metadata.uid"}}}],
        "securityContext":security,
        "resources":{"requests":{"cpu":"10m","memory":"32Mi"},"limits":{"cpu":"250m","memory":"128Mi"}},
        "volumeMounts":[{"name":"output","mountPath":OUTPUT,"readOnly":true}]
    })).map_err(|_| invalid())?;
    serde_json::from_value(json!({
        "apiVersion":"v1", "kind":"Pod",
        "metadata":{
            "name":name(request)?, "namespace":namespace,
            "labels":{LABEL:controller,"app.kubernetes.io/name":"cma-snapshot-export"},
            "annotations":{
                REQUEST:serde_json::to_string(request).map_err(|_| invalid())?,
                DIGEST:request.computed_sha256()?,
                INPUT:serde_json::to_string(&request.export).map_err(|_| invalid())?
            }
        },
        "spec":{
            "restartPolicy":"Never", "activeDeadlineSeconds":7200, "terminationGracePeriodSeconds":1,
            "runtimeClassName":"gvisor", "nodeSelector":{"sandbox.gke.io/runtime":"gvisor"},
            "tolerations":[{"key":"sandbox.gke.io/runtime","operator":"Equal","value":"gvisor","effect":"NoSchedule"}],
            "serviceAccountName":"default", "automountServiceAccountToken":false, "enableServiceLinks":false,
            "securityContext":{"runAsNonRoot":true,"runAsUser":10001,"runAsGroup":10001,"fsGroup":10001,"fsGroupChangePolicy":"OnRootMismatch","seccompProfile":{"type":"RuntimeDefault"}},
            "initContainers":[export], "containers":[transfer],
            "volumes":[
                {"name":"snapshot","persistentVolumeClaim":{"claimName":request.view.volume_reference,"readOnly":true}},
                {"name":"input","downwardAPI":{"defaultMode":292,"items":[{"path":"request.json","fieldRef":{"apiVersion":"v1","fieldPath":format!("metadata.annotations['{INPUT}']")}}]}},
                {"name":"output","emptyDir":{"sizeLimit":"17Gi"}}
            ]
        }
    })).map_err(|_| invalid())
}

pub(super) fn invalid() -> AdapterError {
    AdapterError::Rejected(
        "snapshot exporter Pod identity or execution contract is inconsistent".into(),
    )
}
