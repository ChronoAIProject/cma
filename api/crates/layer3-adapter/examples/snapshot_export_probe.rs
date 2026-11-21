//! Explicit, isolated CSI/export acceptance using only generated fixture data.
//! Usage: CONTEXT NAMESPACE SEED_NAME SEED_UID PINNED_RUNTIME_IMAGE

mod snapshot_export_probe_support;
use snapshot_export_probe_support::{execute, output, remove_pod, until, Error, Result, Sink};

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use k8s_openapi::api::core::v1::Pod;
use kube::{
    api::{Api, AttachedProcess, LogParams, PostParams},
    config::KubeConfigOptions,
};
use layer3_adapter::{provider::PersistentHomeVolumes, *};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

struct Probe {
    store: PersistentHomeVolumes,
    pods: Api<Pod>,
    namespace: String,
    identity: String,
    image: String,
    home: HomeVolumeRequest,
    snapshot: Option<HomeSnapshotRequest>,
    export: Option<HomeSnapshotExportRequest>,
    job: Option<HomeSnapshotExportJobRequest>,
    writer: Option<AttachedProcess>,
    stage: &'static str,
}

impl Probe {
    fn stage(&mut self, stage: &'static str) {
        self.stage = stage;
        println!("{}", json!({"stage":stage,"identity":self.identity}));
    }

    async fn run(&mut self) -> Result<Value> {
        self.stage("allocate-source");
        let receipt = self.store.prepare(&self.home).await?;
        let home = HomeVolumeAttachment {
            request: self.home.clone(),
            receipt,
        };
        let desired = snapshot_export_probe_support::source_pod(
            &self.namespace,
            &self.identity,
            &self.image,
            &home,
        )?;
        let allocated = self.pods.create(&PostParams::default(), &desired).await?;
        let source_uid = allocated
            .metadata
            .uid
            .ok_or("allocated source UID missing")?;
        let source = until(|| async {
            let pod = self.pods.get(&self.identity).await?;
            if pod.metadata.uid.as_deref() != Some(&source_uid) {
                return Err("source Pod was replaced before preparation".into());
            }
            match pod
                .status
                .as_ref()
                .and_then(|status| status.phase.as_deref())
            {
                Some("Running") => Ok(Some(pod)),
                Some("Failed" | "Succeeded") => Err("source runtime terminated".into()),
                _ => Ok(None),
            }
        })
        .await?;
        self.stage("prepare-fixture");
        let install = serde_json::to_string(&wire::RequestFrame::Install(InstallRequest::new(
            "export-probe-install",
            vec![],
        )))?;
        let prepared = output(
            execute(
                &self.pods,
                &source,
                include_str!("snapshot_export_probe_support/setup.js"),
                &install,
            )
            .await?,
        )
        .await?;
        if prepared != b"prepared\n" {
            return Err("source fixture was not prepared".into());
        }
        let snapshot = HomeSnapshotRequest {
            capture_id: self.identity.clone(),
            epoch: 1,
            home,
            snapshot_class: "cma-home-v1".into(),
        };
        self.snapshot = Some(snapshot.clone());
        self.stage("capture-snapshot");
        let initial = self.store.prepare_snapshot(&snapshot).await?;
        let captured = observe_capture(&self.store, &snapshot, initial, false).await?;
        let copying_when_writer_started = !captured.ready;
        self.stage("write-live-source");
        self.writer = Some(
            execute(
                &self.pods,
                &source,
                include_str!("snapshot_export_probe_support/writer.js"),
                "",
            )
            .await?,
        );
        let ready = observe_capture(&self.store, &snapshot, captured, true).await?;
        let export = HomeSnapshotExportRequest {
            snapshot,
            observation: ready,
            runtime_generation: 1,
            conversations: vec![HomeSnapshotConversationIdentity {
                agent_id: "agt_probe".into(),
                conversation_id: "probe-conversation".into(),
            }],
        };
        self.export = Some(export.clone());
        self.stage("allocate-export-view");
        let view = self.store.prepare_view(&export).await?;
        let job = HomeSnapshotExportJobRequest {
            export,
            view: view.receipt,
            attempt: 1,
            image: self.image.clone(),
        };
        self.job = Some(job.clone());
        self.stage("run-exporter");
        let mut observed = self.store.prepare_export_job(&job).await?;
        let deadline = Instant::now() + Duration::from_secs(480);
        loop {
            match observed.phase {
                HomeSnapshotExportJobPhase::Ready => break,
                HomeSnapshotExportJobPhase::Failed => return Err("exporter failed".into()),
                _ => (),
            }
            if Instant::now() >= deadline {
                return Err("exporter observation deadline exceeded".into());
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
            observed = self.store.observe_export_job(&job, &observed).await?;
        }
        self.stage("transfer-archive");
        let sink = Sink::default();
        let receipt = self.store.upload_export(&job, &observed, &sink).await?;
        let archive_bytes = sink.verify()?;
        let transcript =
            b"{\"payload\":{\"id\":\"probe-conversation\"}}\n{\"event\":\"captured reply\"}\n";
        if receipt.conversations.len() != 1
            || receipt.conversations[0].archive_sha256
                != format!("{:x}", Sha256::digest(transcript))
        {
            return Err("captured conversation differs from fixture".into());
        }
        self.stage("verify-concurrent-writes");
        output(execute(&self.pods, &source, "if(process.env.CMA_PROBE_POD_UID!==process.argv[1])process.exit(75);require('node:fs').writeFileSync('/home/cma/export-probe/stop','stop')", "").await?).await?;
        let writer: Value =
            serde_json::from_slice(&output(self.writer.take().ok_or("writer missing")?).await?)?;
        let pod = self.pods.get(&observed.receipt.pod_reference).await?;
        if pod.metadata.uid.as_deref() != Some(&observed.receipt.pod_generation) {
            return Err("exporter changed before verification".into());
        }
        let terminated = pod
            .status
            .as_ref()
            .and_then(|status| status.init_container_statuses.as_ref())
            .and_then(|statuses| statuses.first())
            .and_then(|status| status.state.as_ref())
            .and_then(|state| state.terminated.as_ref())
            .ok_or("export timestamps missing")?;
        let started = terminated
            .started_at
            .as_ref()
            .ok_or("export start missing")?
            .0
            .as_millisecond();
        let finished = terminated
            .finished_at
            .as_ref()
            .ok_or("export finish missing")?
            .0
            .as_millisecond();
        let writes = writer["ticks"]
            .as_array()
            .ok_or("writer ticks missing")?
            .iter()
            .filter(|tick| {
                tick.as_i64()
                    .is_some_and(|stamp| stamp > started && stamp < finished)
            })
            .count();
        if writes < 2 {
            return Err("writer did not advance during export".into());
        }
        Ok(
            json!({"scope":"isolated-real-csi-export-with-generated-fixture","archiveBytes":archive_bytes,"exportMilliseconds":finished-started,"concurrentWrites":writes,"copyingWhenWriterStarted":copying_when_writer_started,"captureTime":job.export.observation.captured_at,"image":self.image}),
        )
    }

    async fn cleanup(&mut self) -> Result<()> {
        self.writer.take();
        if let Some(job) = &self.job {
            let digest = job.computed_sha256()?;
            remove_pod(
                &self.pods,
                &format!("cma-export-{}", &digest[..48]),
                "cma/home-export-digest",
                &digest,
            )
            .await?;
        }
        if let Some(export) = &self.export {
            until(|| async {
                match self.store.remove_view(export, None).await {
                    Ok(()) => Ok(Some(())),
                    Err(AdapterError::Unavailable(_)) => Ok(None),
                    Err(error) => Err(error.into()),
                }
            })
            .await?;
        }
        if let Some(snapshot) = &self.snapshot {
            until(|| async {
                match self.store.remove_snapshot(snapshot, None).await {
                    Ok(()) => Ok(Some(())),
                    Err(AdapterError::Unavailable(_)) => Ok(None),
                    Err(error) => Err(error.into()),
                }
            })
            .await?;
        }
        remove_pod(
            &self.pods,
            &self.identity,
            "cma/export-proof",
            &self.identity,
        )
        .await?;
        until(|| async {
            match self.store.remove(&self.home, None).await {
                Ok(()) => Ok(Some(())),
                Err(AdapterError::Unavailable(_)) => Ok(None),
                Err(error) => Err(error.into()),
            }
        })
        .await?;
        if !self.store.inventory().await?.is_empty()
            || !self.store.snapshot_inventory().await?.is_empty()
            || !self.store.view_inventory().await?.is_empty()
            || !self.store.export_job_inventory().await?.is_empty()
        {
            return Err("probe allocations remain after cleanup".into());
        }
        Ok(())
    }

    async fn failure_evidence(&self) -> Result<Value> {
        let Some(job) = &self.job else {
            return Ok(Value::Null);
        };
        let digest = job.computed_sha256()?;
        let name = format!("cma-export-{}", &digest[..48]);
        let Some(pod) = self.pods.get_opt(&name).await? else {
            return Ok(Value::Null);
        };
        if pod
            .metadata
            .annotations
            .as_ref()
            .and_then(|values| values.get("cma/home-export-digest"))
            != Some(&digest)
        {
            return Err("probe diagnostics target changed authority".into());
        }
        // This probe contains only the image seed and generated data, never user
        // credentials or an existing Sandbox. Keep its diagnostics bounded.
        let logs = self
            .pods
            .logs(
                &name,
                &LogParams {
                    container: Some("export".into()),
                    tail_lines: Some(20),
                    limit_bytes: Some(8192),
                    ..LogParams::default()
                },
            )
            .await
            .ok();
        Ok(json!({"pod":name,"status":pod.status,"exportLog":logs}))
    }
}

async fn observe_capture(
    store: &PersistentHomeVolumes,
    request: &HomeSnapshotRequest,
    mut observation: HomeSnapshotObservation,
    require_ready: bool,
) -> Result<HomeSnapshotObservation> {
    let deadline = Instant::now() + Duration::from_secs(480);
    loop {
        if observation.captured_at.is_some() && (!require_ready || observation.ready) {
            return Ok(observation);
        }
        if Instant::now() >= deadline {
            return Err("snapshot observation deadline exceeded".into());
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
        observation = store.observe_snapshot(request, &observation).await?;
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 5 {
        return Err("usage: CONTEXT NAMESPACE SEED_NAME SEED_UID PINNED_RUNTIME_IMAGE".into());
    }
    let config = kube::Config::from_kubeconfig(&KubeConfigOptions {
        context: Some(args[0].clone()),
        ..KubeConfigOptions::default()
    })
    .await?;
    let api_authority = config.cluster_url.to_string();
    let client = kube::Client::try_from(config)?;
    let identity = format!(
        "cma-export-proof-{}",
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    );
    let mut probe = Probe {
        store: PersistentHomeVolumes::namespaced(
            client.clone(),
            &args[1],
            &identity,
            &api_authority,
        )?,
        pods: Api::namespaced(client, &args[1]),
        namespace: args[1].clone(),
        identity: identity.clone(),
        image: args[4].clone(),
        home: HomeVolumeRequest {
            scope: None,
            volume_id: identity.clone(),
            sandbox_id: identity.clone(),
            policy: HomeVolumePolicy {
                mode: Default::default(),
                storage_class: "cma-home-v1".into(),
                capacity_gib: 1,
                template_name: args[2].clone(),
                template_generation: args[3].clone(),
            },
        },
        snapshot: None,
        export: None,
        job: None,
        writer: None,
        stage: "initialize",
    };
    let result = probe.run().await;
    let stage = probe.stage;
    let diagnostic = if result.is_err() {
        probe.failure_evidence().await.ok()
    } else {
        None
    };
    probe.stage("cleanup");
    let cleanup = probe.cleanup().await;
    println!(
        "{}",
        json!({"result":if result.is_ok() && cleanup.is_ok() { "pass" } else { "fail" },"stage":stage,"identity":identity,"evidence":result.as_ref().ok(),"failure":result.as_ref().err().map(ToString::to_string),"diagnostic":diagnostic,"cleanup":cleanup.as_ref().err().map(ToString::to_string)})
    );
    cleanup?;
    result
        .map(|_| ())
        .map_err(|_| -> Error { "isolated snapshot export probe failed".into() })
}
