//! A private Unix socket for one pinned qualification plan per process.

use super::{ArmedControl, ReachedControl, Registry, MAX_LIFETIME};
use layer2_core::{
    agent_ownership::{OwnershipCoordinator, OwnershipToken},
    qualification::{ControlContext, ScheduleEffect},
    AgentId,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::{self, Read},
    os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    task::JoinHandle,
    time::timeout,
};

const MAX_FRAME: usize = 2048;
const IO_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PlanDocument {
    version: u16,
    build: String,
    request_digest: [u8; 32],
    agent_digest: [u8; 32],
    lifetime_ms: u64,
    owner: Option<OwnerDocument>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnerDocument {
    agent: String,
    instance: String,
    epoch: u64,
    protocol: u32,
}

pub struct Plan {
    digest: [u8; 32],
    context: ControlContext,
    lifetime: Duration,
    owner: Option<OwnershipToken>,
}

impl Plan {
    pub fn open(path: &Path, build: &str) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.uid() != effective_owner()
            || metadata.mode() & 0o077 != 0
        {
            return Err(refusal("qualification plan permissions refused"));
        }
        let mut bytes = Vec::new();
        file.take((MAX_FRAME + 1) as u64).read_to_end(&mut bytes)?;
        Self::parse(&bytes, build)
    }

    fn parse(bytes: &[u8], build: &str) -> io::Result<Self> {
        if bytes.len() > MAX_FRAME {
            return Err(refusal("qualification plan exceeds its bound"));
        }
        let plan: PlanDocument = serde_json::from_slice(bytes)
            .map_err(|_| refusal("qualification plan schema refused"))?;
        if plan.version != 1
            || plan.build != build
            || build.len() != 40
            || !build
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(refusal("qualification plan build identity refused"));
        }
        let lifetime = Duration::from_millis(plan.lifetime_ms);
        if lifetime.is_zero() || lifetime > MAX_LIFETIME {
            return Err(refusal("qualification plan lifetime refused"));
        }
        let owner = plan.owner.map(|owner| OwnershipToken {
            agent_id: AgentId(owner.agent),
            instance_id: owner.instance,
            epoch: owner.epoch,
            protocol: owner.protocol,
        });
        Ok(Self {
            digest: Sha256::digest(bytes).into(),
            context: ControlContext::from_digests(plan.request_digest, plan.agent_digest),
            lifetime,
            owner,
        })
    }
}

pub struct ControlServer {
    _socket: SocketPath,
    task: JoinHandle<()>,
}

impl ControlServer {
    pub fn bind(
        path: PathBuf,
        plan: Plan,
        registry: Registry,
        coordinator: Option<Arc<OwnershipCoordinator>>,
    ) -> io::Result<Self> {
        validate_parent(&path)?;
        if plan.owner.is_some() && coordinator.is_none() {
            return Err(refusal("qualification owner selector has no coordinator"));
        }
        let listener = UnixListener::bind(&path)?;
        let socket = SocketPath::own(path)?;
        fs::set_permissions(&socket.path, fs::Permissions::from_mode(0o600))?;
        let task = tokio::spawn(async move {
            let mut used = false;
            while let Ok((stream, _)) = listener.accept().await {
                if stream
                    .peer_cred()
                    .is_ok_and(|peer| peer.uid() == effective_owner())
                {
                    let _ =
                        serve(stream, &plan, &registry, coordinator.as_deref(), &mut used).await;
                }
            }
        });
        Ok(Self {
            _socket: socket,
            task,
        })
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct SocketPath {
    path: PathBuf,
    device: u64,
    inode: u64,
}

impl SocketPath {
    fn own(path: PathBuf) -> io::Result<Self> {
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.file_type().is_socket() || metadata.uid() != effective_owner() {
            return Err(refusal("qualification socket ownership refused"));
        }
        Ok(Self {
            path,
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }

    fn remove(&self) {
        if fs::symlink_metadata(&self.path).is_ok_and(|metadata| {
            metadata.file_type().is_socket()
                && (metadata.dev(), metadata.ino()) == (self.device, self.inode)
        }) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

impl Drop for SocketPath {
    fn drop(&mut self) {
        self.remove();
    }
}

fn validate_parent(socket: &Path) -> io::Result<()> {
    if !socket.is_absolute()
        || socket
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err(refusal("qualification socket path refused"));
    }
    let parent = socket
        .parent()
        .ok_or_else(|| refusal("qualification socket parent missing"))?;
    let metadata = parent.symlink_metadata()?;
    if !metadata.is_dir() || metadata.uid() != effective_owner() || metadata.mode() & 0o077 != 0 {
        return Err(refusal(
            "qualification socket directory permissions refused",
        ));
    }
    Ok(())
}

fn effective_owner() -> u32 {
    // geteuid has no arguments and does not access filesystem data.
    unsafe { libc::geteuid() }
}

#[derive(Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
struct Request {
    sequence: u8,
    plan_digest: [u8; 32],
    action: Action,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Action {
    Arm,
    Wait,
    CancelOwner,
    Release,
}

#[derive(Clone, Copy, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum Reply {
    Armed,
    Reached {
        agent_revision: u64,
        scheduled: bool,
    },
    Cancelled,
    Released,
    Refused,
}

struct Session {
    arm: Option<ArmedControl>,
    reached: Option<ReachedControl>,
}

async fn serve(
    mut stream: UnixStream,
    plan: &Plan,
    registry: &Registry,
    coordinator: Option<&OwnershipCoordinator>,
    used: &mut bool,
) -> io::Result<()> {
    let mut session = Session {
        arm: None,
        reached: None,
    };
    let mut history = Vec::<(Request, Reply)>::new();
    loop {
        let read_budget = if session.arm.is_some() {
            plan.lifetime.max(IO_TIMEOUT)
        } else {
            IO_TIMEOUT
        };
        let request = timeout(read_budget, read_request(&mut stream)).await??;
        if request.plan_digest != plan.digest {
            return Err(refusal("qualification request plan mismatch"));
        }
        let sequence = usize::from(request.sequence);
        if let Some((original, reply)) = history.get(sequence) {
            if request != *original {
                return Err(refusal("qualification command replay changed"));
            }
            timeout(IO_TIMEOUT, write_reply(&mut stream, *reply)).await??;
            continue;
        }
        if sequence != history.len() || sequence > 7 {
            return Err(refusal("qualification command sequence refused"));
        }
        let reply = match request.action {
            Action::Arm if session.arm.is_none() && !*used => {
                let result = match (&plan.owner, coordinator) {
                    (Some(token), Some(coordinator)) => {
                        match coordinator.capture_qualification_owner(token).await {
                            Some(owner) => registry
                                .arm_with_owner(plan.context, plan.lifetime, owner)
                                .ok(),
                            None => None,
                        }
                    }
                    (None, _) => registry.arm(plan.context, plan.lifetime).ok(),
                    _ => None,
                };
                if let Some(arm) = result {
                    *used = true;
                    session.arm = Some(arm);
                    Reply::Armed
                } else {
                    Reply::Refused
                }
            }
            Action::Wait if session.reached.is_none() => {
                if let Some(arm) = session.arm.as_mut() {
                    let reached = tokio::select! {
                        biased;
                        _ = stream.read_u8() => return Ok(()),
                        reached = arm.reached() => reached,
                    };
                    if let Some(receipt) = reached {
                        let reply = Reply::Reached {
                            agent_revision: receipt.schedule.agent_revision,
                            scheduled: receipt.schedule.effect == ScheduleEffect::Scheduled,
                        };
                        session.reached = Some(receipt);
                        reply
                    } else {
                        Reply::Refused
                    }
                } else {
                    Reply::Refused
                }
            }
            Action::CancelOwner => match (&mut session.arm, &session.reached) {
                (Some(arm), Some(reached)) => {
                    if arm.cancel_owner(reached) {
                        Reply::Cancelled
                    } else {
                        Reply::Refused
                    }
                }
                _ => Reply::Refused,
            },
            Action::Release => {
                if let Some(arm) = session.arm.take() {
                    arm.release();
                    Reply::Released
                } else {
                    Reply::Refused
                }
            }
            _ => Reply::Refused,
        };
        timeout(IO_TIMEOUT, write_reply(&mut stream, reply)).await??;
        history.push((request, reply));
    }
}

async fn read_request(stream: &mut UnixStream) -> io::Result<Request> {
    let length = stream.read_u32().await? as usize;
    if !(1..=MAX_FRAME).contains(&length) {
        return Err(refusal("qualification frame size refused"));
    }
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes).await?;
    serde_json::from_slice(&bytes).map_err(|_| refusal("qualification command schema refused"))
}

async fn write_reply(stream: &mut UnixStream, reply: Reply) -> io::Result<()> {
    let bytes = serde_json::to_vec(&reply).map_err(io::Error::other)?;
    stream.write_u32(bytes.len() as u32).await?;
    stream.write_all(&bytes).await
}

fn refusal(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests;
