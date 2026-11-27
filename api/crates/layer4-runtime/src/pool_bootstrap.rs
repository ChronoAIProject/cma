//! Native pool Pods have no network listener or tenant identity until assigned.
//! Kubernetes exec forwards the assignment through stdin to this local socket.
use std::collections::BTreeMap;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use layer3_adapter::runtime_bootstrap::{
    valid_pod_uid, RuntimeBootstrapReceipt, RuntimeBootstrapRequest, StagingStartupFailure,
    MAX_RUNTIME_BOOTSTRAP_BYTES, RUNTIME_BOOTSTRAP_SOCKET,
};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{oneshot, Mutex};
use tokio::task::{JoinHandle, JoinSet};

use crate::{config::RuntimeConfig, control_auth::ControlAuth};

const IO_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_CONNECTIONS: usize = 8;

pub(crate) struct AssignedRuntime {
    pub config: RuntimeConfig,
    pub auth: Arc<ControlAuth>,
    pub startup_failure: Option<(StagingStartupFailure, oneshot::Receiver<()>)>,
}

pub(crate) struct AssignmentServer {
    admission: Arc<Mutex<Admission>>,
    task: JoinHandle<()>,
    path: PathBuf,
    inode: u64,
}

impl Drop for AssignmentServer {
    fn drop(&mut self) {
        self.task.abort();
        if std::fs::symlink_metadata(&self.path).is_ok_and(|m| m.ino() == self.inode) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
enum Reply {
    Accepted { receipt: RuntimeBootstrapReceipt },
    Rejected,
}

struct Admission {
    receipt: Option<RuntimeBootstrapReceipt>,
    disarmed_receipt: Option<RuntimeBootstrapReceipt>,
    sender: Option<oneshot::Sender<AssignedRuntime>>,
    startup_confirmation: Option<oneshot::Sender<()>>,
    startup_exit_committed: bool,
}

impl AssignmentServer {
    async fn start(
        path: PathBuf,
        pod_uid: String,
        base: BTreeMap<String, String>,
    ) -> Result<(Self, oneshot::Receiver<AssignedRuntime>), String> {
        if !valid_pod_uid(&pod_uid)
            || !path.is_absolute()
            || path.components().any(|p| {
                matches!(
                    p,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            })
            || base.contains_key("CHRONO_RUNTIME_CONTROL_TOKEN")
            || base.contains_key("CHRONO_SANDBOX_ID")
        {
            return Err("invalid unassigned runtime configuration".into());
        }
        prepare_socket(&path).await?;
        let listener =
            UnixListener::bind(&path).map_err(|_| "cannot bind runtime assignment socket")?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .map_err(|_| "cannot protect runtime assignment socket")?;
        let inode = std::fs::symlink_metadata(&path)
            .map_err(|_| "cannot inspect runtime assignment socket")?
            .ino();
        let (sender, receiver) = oneshot::channel();
        let admission = Arc::new(Mutex::new(Admission {
            receipt: None,
            disarmed_receipt: None,
            sender: Some(sender),
            startup_confirmation: None,
            startup_exit_committed: false,
        }));
        let socket_path = path.clone();
        let server_admission = admission.clone();
        let task = tokio::spawn(async move {
            let mut jobs = JoinSet::new();
            let base = Arc::new(base);
            loop {
                tokio::select! {
                    _ = jobs.join_next(), if !jobs.is_empty() => {},
                    connected = listener.accept(), if jobs.len() < MAX_CONNECTIONS => {
                        let Ok((stream, _)) = connected else { break; };
                        let (admission, base, pod_uid, path) =
                            (admission.clone(), base.clone(), pod_uid.clone(), socket_path.clone());
                        jobs.spawn(async move {
                            // Failed requests produce no payload-bearing logs or persistent files.
                            let _ = tokio::time::timeout(IO_TIMEOUT, serve(stream, admission, &pod_uid, &base, &path)).await;
                        });
                    }
                }
            }
        });
        Ok((
            Self {
                admission: server_admission,
                task,
                path,
                inode,
            },
            receiver,
        ))
    }
}

async fn prepare_socket(path: &Path) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or("invalid runtime assignment directory")?;
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|_| "cannot prepare runtime assignment directory")?;
    match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) => {
            if !metadata.file_type().is_socket() || metadata.uid() != unsafe { libc::geteuid() } {
                return Err("runtime assignment path is not an owned socket".into());
            }
            match UnixStream::connect(path).await {
                Ok(_) => return Err("runtime assignment socket is already serving".into()),
                Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
                    tokio::fs::remove_file(path)
                        .await
                        .map_err(|_| "cannot remove stale runtime assignment socket")?;
                }
                Err(_) => return Err("runtime assignment socket is unavailable".into()),
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err("cannot inspect runtime assignment path".into()),
    }
    Ok(())
}

async fn serve(
    mut stream: UnixStream,
    admission: Arc<Mutex<Admission>>,
    pod_uid: &str,
    base: &BTreeMap<String, String>,
    socket: &Path,
) -> Result<(), String> {
    let request = read_bounded(&mut stream, MAX_RUNTIME_BOOTSTRAP_BYTES)
        .await
        .and_then(|bytes| {
            serde_json::from_slice::<RuntimeBootstrapRequest>(&bytes)
                .map_err(|_| "invalid runtime assignment".into())
        });
    let Ok(request) = request else {
        return write_reply(&mut stream, Reply::Rejected).await;
    };
    if request.pod_uid != pod_uid {
        return write_reply(&mut stream, Reply::Rejected).await;
    }
    if request.staging_startup_confirmed {
        return confirm_startup(request, &admission, &mut stream).await;
    }
    let reply = match accept(request, &admission, base, socket).await {
        Ok(receipt) => Reply::Accepted { receipt },
        Err(_) => Reply::Rejected,
    };
    write_reply(&mut stream, reply).await
}

async fn write_reply(stream: &mut UnixStream, reply: Reply) -> Result<(), String> {
    let bytes =
        serde_json::to_vec(&reply).map_err(|_| "cannot encode runtime assignment receipt")?;
    stream
        .write_all(&bytes)
        .await
        .map_err(|_| "cannot acknowledge runtime assignment")?;
    stream
        .shutdown()
        .await
        .map_err(|_| "cannot finish runtime assignment".into())
}

async fn confirm_startup(
    mut request: RuntimeBootstrapRequest,
    admission: &Mutex<Admission>,
    stream: &mut UnixStream,
) -> Result<(), String> {
    let Ok(receipt) = RuntimeBootstrapReceipt::for_request(&request) else {
        return write_reply(stream, Reply::Rejected).await;
    };
    request.staging_startup_confirmed = false;
    let Ok(original) = RuntimeBootstrapReceipt::for_request(&request) else {
        return write_reply(stream, Reply::Rejected).await;
    };
    // Keep confirmation delivery and disarm in the same critical section.
    // A failed reply leaves the assignment eligible for disarm.
    let mut state = admission.lock().await;
    if state.receipt.as_ref() != Some(&original) {
        return write_reply(stream, Reply::Rejected).await;
    }
    write_reply(stream, Reply::Accepted { receipt }).await?;
    if let Some(sender) = state.startup_confirmation.take() {
        state.startup_exit_committed = sender.send(()).is_ok();
    }
    Ok(())
}

async fn accept(
    mut request: RuntimeBootstrapRequest,
    admission: &Mutex<Admission>,
    base: &BTreeMap<String, String>,
    socket: &Path,
) -> Result<RuntimeBootstrapReceipt, String> {
    let receipt =
        RuntimeBootstrapReceipt::for_request(&request).map_err(|_| "invalid runtime assignment")?;
    let mut state = admission.lock().await;
    if let Some(current) = &state.receipt {
        if current == &receipt {
            return Ok(receipt);
        }
        if !state.startup_exit_committed && state.disarmed_receipt.as_ref() == Some(&receipt) {
            // A manager can replay the same assignment after its staging
            // switch has been removed or expired. Drop the confirmation
            // sender so a pending Runtime can continue startup immediately.
            state.startup_confirmation = None;
            state.disarmed_receipt = None;
            state.receipt = Some(receipt.clone());
            return Ok(receipt);
        }
        return Err("runtime is already assigned".into());
    }
    let fault = request.staging_startup_failure.take();
    let disarmed_receipt = if fault.is_some() {
        Some(
            RuntimeBootstrapReceipt::for_request(&request)
                .map_err(|_| "invalid disarmed runtime assignment")?,
        )
    } else {
        None
    };
    let token = request
        .environment
        .remove("CHRONO_RUNTIME_CONTROL_TOKEN")
        .ok_or("runtime control token is absent")?;
    let auth = ControlAuth::from_token(token)?;
    let mut environment = base.clone();
    environment.extend(request.environment);
    environment.remove("CHRONO_RUNTIME_POD_UID");
    environment.remove("CHRONO_RUNTIME_BOOTSTRAP_SOCKET");
    let mut config = RuntimeConfig::from_values(environment)?;
    // Managed Standard runtimes supply the seal. Agent Manager still gates
    // its use on the qualified running digest and the current writer.
    if !config.machine_included {
        config
            .process_env
            .entry(crate::bridge::STALE_TURN_SEAL_FLAG.into())
            .or_insert_with(|| "1".into());
    }
    if socket.starts_with(&config.home) {
        return Err("runtime assignment socket must be outside the tenant home".into());
    }
    let startup_failure = fault.map(|fault| {
        let (confirm, wait) = oneshot::channel();
        state.startup_confirmation = Some(confirm);
        (fault, wait)
    });
    state
        .sender
        .take()
        .ok_or("runtime assignment receiver is unavailable")?
        .send(AssignedRuntime {
            config,
            auth,
            startup_failure,
        })
        .map_err(|_| "runtime assignment receiver is unavailable")?;
    state.disarmed_receipt = disarmed_receipt;
    state.receipt = Some(receipt.clone());
    Ok(receipt)
}

fn socket_path() -> PathBuf {
    std::env::var_os("CHRONO_RUNTIME_BOOTSTRAP_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(RUNTIME_BOOTSTRAP_SOCKET))
}

pub(crate) async fn wait_for_assignment() -> Result<(AssignedRuntime, AssignmentServer), String> {
    let pod_uid =
        std::env::var("CHRONO_RUNTIME_POD_UID").map_err(|_| "runtime Pod UID is absent")?;
    let (server, receiver) =
        AssignmentServer::start(socket_path(), pod_uid, std::env::vars().collect()).await?;
    tokio::select! {
        runtime = receiver => runtime.map(|runtime| (runtime, server)).map_err(|_| "runtime assignment stopped".into()),
        () = crate::shutdown_signal() => Err("runtime assignment interrupted".into()),
    }
}

pub(crate) async fn assign_from_stdio() -> Result<(), String> {
    let bytes = tokio::time::timeout(
        IO_TIMEOUT,
        tokio::task::spawn_blocking(|| {
            use std::io::Read;
            let mut bytes = Vec::new();
            std::io::stdin()
                .take(MAX_RUNTIME_BOOTSTRAP_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| "cannot read runtime assignment")?;
            if bytes.len() > MAX_RUNTIME_BOOTSTRAP_BYTES {
                return Err("runtime assignment exceeds its bound");
            }
            Ok(bytes)
        }),
    )
    .await
    .map_err(|_| "runtime assignment input timed out")?
    .map_err(|_| "runtime assignment input task failed")??;
    let request: RuntimeBootstrapRequest =
        serde_json::from_slice(&bytes).map_err(|_| "invalid runtime assignment input")?;
    let receipt = assign(&socket_path(), &request).await?;
    let mut output =
        serde_json::to_vec(&receipt).map_err(|_| "cannot encode runtime assignment receipt")?;
    output.push(b'\n');
    std::io::Write::write_all(&mut std::io::stdout(), &output)
        .map_err(|_| "cannot write runtime assignment receipt".into())
}

async fn assign(
    path: &Path,
    request: &RuntimeBootstrapRequest,
) -> Result<RuntimeBootstrapReceipt, String> {
    request
        .validate()
        .map_err(|_| "invalid runtime assignment input")?;
    let bytes = serde_json::to_vec(request).map_err(|_| "invalid runtime assignment input")?;
    tokio::time::timeout(IO_TIMEOUT, async {
        let mut stream = UnixStream::connect(path)
            .await
            .map_err(|_| "runtime assignment socket is unavailable")?;
        stream
            .write_all(&bytes)
            .await
            .map_err(|_| "cannot send runtime assignment")?;
        stream
            .shutdown()
            .await
            .map_err(|_| "cannot finish runtime assignment input")?;
        let bytes = read_bounded(stream, 1024).await?;
        match serde_json::from_slice::<Reply>(&bytes)
            .map_err(|_| "invalid runtime assignment acknowledgement")?
        {
            Reply::Accepted { receipt } => {
                receipt
                    .validate_for(request)
                    .map_err(|_| "runtime assignment receipt mismatch")?;
                Ok(receipt)
            }
            Reply::Rejected => Err("runtime assignment was rejected".into()),
        }
    })
    .await
    .map_err(|_| "runtime assignment timed out")?
}

async fn read_bounded(reader: impl AsyncRead + Unpin, limit: usize) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    reader
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| "cannot read runtime assignment")?;
    if bytes.len() > limit {
        return Err("runtime assignment exceeds its bound".into());
    }
    Ok(bytes)
}

#[cfg(test)]
#[path = "pool_bootstrap_tests.rs"]
mod tests;

pub(crate) async fn apply_staging_startup_failure(
    server: &AssignmentServer,
    fault: StagingStartupFailure,
    confirmed: oneshot::Receiver<()>,
) -> Result<(), String> {
    use layer3_adapter::runtime_bootstrap::NATIVE_ASSIGNMENT_DEADLINE;
    if fault.armed_at(std::time::SystemTime::now())
        && matches!(
            tokio::time::timeout(
                NATIVE_ASSIGNMENT_DEADLINE + Duration::from_secs(10),
                confirmed
            )
            .await,
            Ok(Ok(()))
        )
    {
        // Give the confirmation relay time to finish before terminating the process.
        tokio::time::sleep(Duration::from_secs(1)).await;
        if fault.armed_at(std::time::SystemTime::now()) {
            return Err(
                "staging qualification: runtime startup interrupted after assignment".into(),
            );
        }
    }
    // An unconfirmed or expired fault leaves this process available for ordinary adoption.
    let mut state = server.admission.lock().await;
    state.startup_confirmation = None;
    state.startup_exit_committed = false;
    Ok(())
}
