use std::{
    io::Read,
    sync::Mutex,
    time::{Duration, Instant},
};

use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, AttachParams, AttachedProcess, DeleteParams, Preconditions};
use layer3_adapter::{
    hosted::{
        CheckpointArchiveDownload, CheckpointArchiveReceipt, CheckpointArchiveStore,
        CheckpointArchiveUpload,
    },
    AdapterError, HomeVolumeAttachment,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Result<T> = std::result::Result<T, Error>;

pub async fn until<T, F, Fut>(mut action: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<Option<T>>>,
{
    let deadline = Instant::now() + Duration::from_secs(480);
    loop {
        if let Some(value) = action().await? {
            return Ok(value);
        }
        if Instant::now() >= deadline {
            return Err("probe observation deadline exceeded".into());
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

pub fn source_pod(
    namespace: &str,
    identity: &str,
    image: &str,
    home: &HomeVolumeAttachment,
) -> Result<Pod> {
    Ok(serde_json::from_value(json!({
        "apiVersion":"v1", "kind":"Pod",
        "metadata":{"namespace":namespace,"name":identity,"labels":{"cma/export-proof":identity}},
        "spec":{
            "restartPolicy":"Never","activeDeadlineSeconds":2400,"terminationGracePeriodSeconds":1,
            "runtimeClassName":"gvisor","nodeSelector":{"sandbox.gke.io/runtime":"gvisor"},
            "tolerations":[{"key":"sandbox.gke.io/runtime","operator":"Equal","value":"gvisor","effect":"NoSchedule"}],
            "automountServiceAccountToken":false,"enableServiceLinks":false,
            "securityContext":{"runAsNonRoot":true,"runAsUser":10001,"runAsGroup":10001,"fsGroup":10001,"seccompProfile":{"type":"RuntimeDefault"}},
            "containers":[{
                "name":"source","image":image,"imagePullPolicy":"IfNotPresent",
                "securityContext":{"allowPrivilegeEscalation":false,"readOnlyRootFilesystem":true,"capabilities":{"drop":["ALL"]}},
                "resources":{"requests":{"cpu":"100m","memory":"128Mi"},"limits":{"cpu":"1","memory":"1Gi"}},
                "env":[
                    {"name":"CHRONO_SANDBOX_ID","value":home.request.sandbox_id},
                    {"name":"CHRONO_RUNTIME_HOME_IDENTITY","value":home.identity()?},
                    {"name":"TMPDIR","value":"/var/lib/cma-transfer"},
                    {"name":"CMA_PROBE_POD_UID","valueFrom":{"fieldRef":{"fieldPath":"metadata.uid"}}}
                ],
                "volumeMounts":[
                    {"name":"disk","mountPath":"/home/cma","subPath":"home"},
                    {"name":"disk","mountPath":"/var/lib/cma-runtime","subPath":"state"},
                    {"name":"disk","mountPath":"/var/lib/cma-transfer","subPath":"transfer"},
                    {"name":"tmp","mountPath":"/tmp"}
                ]
            }],
            "volumes":[{"name":"disk","persistentVolumeClaim":{"claimName":home.receipt.storage_reference}},{"name":"tmp","emptyDir":{"sizeLimit":"64Mi"}}]
        }
    }))?)
}

pub async fn execute(
    pods: &Api<Pod>,
    pod: &Pod,
    script: &str,
    argument: &str,
) -> Result<AttachedProcess> {
    Ok(pods
        .exec(
            pod.metadata.name.as_deref().ok_or("source name missing")?,
            [
                "node",
                "-e",
                script,
                pod.metadata.uid.as_deref().ok_or("source UID missing")?,
                argument,
            ],
            &AttachParams::default()
                .container("source")
                .stdin(false)
                .stderr(false)
                .stdout(true)
                .tty(false),
        )
        .await?)
}

pub async fn output(mut process: AttachedProcess) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    process
        .stdout()
        .ok_or("stdout missing")?
        .take(2 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .await?;
    if bytes.len() > 2 * 1024 * 1024 {
        return Err("probe output exceeded its bound".into());
    }
    let status = process.take_status().ok_or("status missing")?;
    let (status, joined) = tokio::join!(status, process.join());
    joined?;
    if status.is_none_or(|status| status.status.as_deref() != Some("Success")) {
        return Err("probe command failed".into());
    }
    Ok(bytes)
}

/// Probe cleanup is confined to its own allocation labels/annotations and the
/// observed UID. It also handles a Pod whose server defaults failed admission.
pub async fn remove_pod(
    pods: &Api<Pod>,
    name: &str,
    identity_key: &str,
    identity: &str,
) -> Result<()> {
    until(|| async {
        let Some(pod) = pods.get_opt(name).await? else {
            return Ok(Some(()));
        };
        let recorded = pod
            .metadata
            .labels
            .as_ref()
            .and_then(|values| values.get(identity_key))
            .or_else(|| {
                pod.metadata
                    .annotations
                    .as_ref()
                    .and_then(|values| values.get(identity_key))
            });
        if recorded.map(String::as_str) != Some(identity) {
            return Err("probe cleanup target changed ownership".into());
        }
        if pod.metadata.deletion_timestamp.is_none() {
            pods.delete(
                name,
                &DeleteParams {
                    preconditions: Some(Preconditions {
                        uid: pod.metadata.uid,
                        resource_version: pod.metadata.resource_version,
                    }),
                    ..DeleteParams::default()
                },
            )
            .await?;
        }
        Ok(None)
    })
    .await
}

#[derive(Default)]
pub struct Sink(Mutex<Vec<u8>>);

#[async_trait::async_trait]
impl CheckpointArchiveStore for Sink {
    async fn upload(
        &self,
        mut archive: CheckpointArchiveUpload,
    ) -> layer3_adapter::Result<CheckpointArchiveReceipt> {
        let mut bytes = Vec::new();
        while let Some(chunk) = archive.chunks.recv().await {
            bytes.extend_from_slice(&chunk?);
            if bytes.len() > 64 * 1024 * 1024 {
                return Err(AdapterError::Rejected(
                    "probe archive exceeded its bound".into(),
                ));
            }
        }
        if bytes.len() as u64 != archive.size_bytes
            || format!("{:x}", Sha256::digest(&bytes)) != archive.sha256
        {
            return Err(AdapterError::Rejected(
                "probe archive integrity failed".into(),
            ));
        }
        *self.0.lock().unwrap() = bytes;
        Ok(CheckpointArchiveReceipt {
            object_reference: archive.object_reference,
            sha256: archive.sha256,
            size_bytes: archive.size_bytes,
        })
    }
    async fn download(&self, _: &str) -> layer3_adapter::Result<CheckpointArchiveDownload> {
        Err(AdapterError::Rejected("probe sink has no download".into()))
    }
}

impl Sink {
    pub fn verify(&self) -> Result<usize> {
        let bytes = self.0.lock().unwrap();
        let mut inflated = Vec::new();
        flate2::read::GzDecoder::new(bytes.as_slice())
            .take(128 * 1024 * 1024 + 1)
            .read_to_end(&mut inflated)?;
        if inflated.len() > 128 * 1024 * 1024 || inflated.get(..16) != Some(b"CMAHOMEARCHIVE1\n") {
            return Err("probe archive header invalid".into());
        }
        let size = u64::from_be_bytes(
            inflated
                .get(16..24)
                .ok_or("manifest length missing")?
                .try_into()?,
        ) as usize;
        let manifest: Value =
            serde_json::from_slice(inflated.get(24..24 + size).ok_or("manifest missing")?)?;
        let entries = manifest["entries"]
            .as_array()
            .ok_or("manifest entries missing")?;
        for (path, expected) in [
            ("export-probe/changed", "captured content"),
            ("export-probe/deleted-later", "captured deletion target"),
        ] {
            let entry = entries
                .iter()
                .find(|entry| entry["path"] == path)
                .ok_or("captured file missing")?;
            let start = 24 + size + entry["offset"].as_u64().ok_or("offset missing")? as usize;
            let end = start + entry["size"].as_u64().ok_or("size missing")? as usize;
            if inflated.get(start..end) != Some(expected.as_bytes()) {
                return Err("archive contains later contents".into());
            }
        }
        if entries
            .iter()
            .any(|entry| entry["path"] == "export-probe/created-later")
        {
            return Err("archive contains a later creation".into());
        }
        Ok(bytes.len())
    }
}
