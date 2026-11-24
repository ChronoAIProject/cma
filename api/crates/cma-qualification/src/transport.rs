//! A private Unix socket for one pinned qualification plan per process.

use super::{Registry, MAX_LIFETIME};
mod session;
use layer2_core::{
    agent_ownership::{OwnershipCoordinator, OwnershipStore},
    qualification::ControlContext,
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
    #[serde(default)]
    mode: Mode,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Mode {
    #[default]
    Control,
    ControlWithOwner,
    Renewal,
}

pub struct Plan {
    digest: [u8; 32],
    context: ControlContext,
    lifetime: Duration,
    mode: Mode,
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
        Ok(Self {
            digest: Sha256::digest(bytes).into(),
            context: ControlContext::from_digests(plan.request_digest, plan.agent_digest),
            lifetime,
            mode: plan.mode,
        })
    }
    pub fn needs_renewal_store(&self) -> bool {
        self.mode == Mode::Renewal
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
        isolated_renewal_store: Option<Arc<dyn OwnershipStore>>,
    ) -> io::Result<Self> {
        validate_parent(&path)?;
        if plan.mode != Mode::Control && coordinator.is_none() {
            return Err(refusal("qualification owner selector has no coordinator"));
        }
        if plan.needs_renewal_store() != isolated_renewal_store.is_some() {
            return Err(refusal("qualification isolated renewal store refused"));
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
                    let _ = session::serve(
                        stream,
                        &plan,
                        &registry,
                        coordinator.as_deref(),
                        isolated_renewal_store.as_ref(),
                        &mut used,
                    )
                    .await;
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

pub fn validate_parent(socket: &Path) -> io::Result<()> {
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
    owner_digest: Option<[u8; 32]>,
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
    Renewal {
        started: bool,
        result: Option<super::RenewalOutcome>,
        caller_dropped: bool,
    },
    Refused,
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
