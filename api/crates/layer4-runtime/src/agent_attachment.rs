//! Publish complete owner uploads without following mutable home symlinks.
use std::ffi::CString;
use std::fs::{File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::os::fd::{AsRawFd as _, FromRawFd as _};
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::path::Path;

use layer3_adapter::{AgentAttachmentReceipt, AgentAttachmentRequest};
use sha2::{Digest as _, Sha256};

use crate::config::RuntimeConfig;
use crate::failure::{Result, RuntimeError};

const RECORD: &str = ".cma-upload.json";

pub(crate) fn supported(config: &RuntimeConfig) -> bool {
    config.home == Path::new("/home/cma")
        && config.home_identity.is_some()
        && std::fs::read_to_string("/proc/self/mountinfo").is_ok_and(|text| mounted_home(&text))
}

fn mounted_home(mounts: &str) -> bool {
    mounts
        .lines()
        .any(|line| line.split_whitespace().nth(4) == Some("/home"))
}

pub(crate) fn publish(
    root: &Path,
    request: &AgentAttachmentRequest,
    content: &[u8],
) -> Result<AgentAttachmentReceipt> {
    request
        .validate_content(content)
        .map_err(|_| rejected("attachment metadata or content is invalid"))?;
    let home = directory(root)?;
    let attachments = child(&home, "attachments", true)?;
    let agent = child(&attachments, request.agent_id.as_str(), true)?;
    let identity = manifest(request)?;
    if let Some(existing) = existing(&agent, request, &identity)? {
        return Ok(existing);
    }
    let stage = Stage::new(&agent, &request.name)?;
    write_new(&stage.directory, &request.name, content)?;
    write_new(&stage.directory, RECORD, &identity)?;
    stage.directory.sync_all().map_err(io_error)?;
    match publish_directory(&agent, &stage.name, &request.attachment_id) {
        Ok(()) => {
            agent.sync_all().map_err(io_error)?;
            // Verify the named, complete result, including an exact replay after
            // another process published first. No temporary path is returned.
            existing(&agent, request, &identity)?
                .ok_or_else(|| rejected("attachment publication disappeared"))
        }
        Err(error) if matches!(error.raw_os_error(), Some(libc::EEXIST | libc::ENOTEMPTY)) => {
            existing(&agent, request, &identity)?
                .ok_or_else(|| rejected("attachment publication conflicts"))
        }
        Err(error) => Err(io_error(error)),
    }
}

fn manifest(request: &AgentAttachmentRequest) -> Result<Vec<u8>> {
    serde_json::to_vec(&serde_json::json!({
        "schema": 1, "sandbox_id": request.sandbox_id, "agent_id": request.agent_id,
        "attachment_id": request.attachment_id, "name": request.name,
        "media_type": request.media_type, "size_bytes": request.size_bytes, "sha256": request.sha256,
    })).map_err(io_error)
}

fn existing(
    parent: &File,
    request: &AgentAttachmentRequest,
    identity: &[u8],
) -> Result<Option<AgentAttachmentReceipt>> {
    let directory = match open_at(
        parent,
        &request.attachment_id,
        libc::O_RDONLY | libc::O_DIRECTORY,
        0,
    ) {
        Ok(directory) => directory,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(rejected("attachment identity is not a real directory")),
    };
    let record = regular(&directory, RECORD)?;
    if record.metadata().map_err(io_error)?.len() != identity.len() as u64 {
        return Err(conflict());
    }
    let mut stored = Vec::new();
    record
        .take(4097)
        .read_to_end(&mut stored)
        .map_err(io_error)?;
    if stored != identity {
        return Err(conflict());
    }
    let mut payload = regular(&directory, &request.name)?;
    if payload.metadata().map_err(io_error)?.len() != request.size_bytes {
        return Err(conflict());
    }
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut remaining = request.size_bytes;
    loop {
        let read = payload.read(&mut buffer).map_err(io_error)?;
        if read == 0 {
            break;
        }
        if read as u64 > remaining {
            return Err(conflict());
        }
        remaining -= read as u64;
        digest.update(&buffer[..read]);
    }
    if remaining != 0 || format!("{:x}", digest.finalize()) != request.sha256 {
        return Err(conflict());
    }
    Ok(Some(AgentAttachmentReceipt::for_request(request)))
}

fn directory(path: &Path) -> Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|_| rejected("attachment home is not a real directory"))
}

fn child(parent: &File, name: &str, create: bool) -> Result<File> {
    if create {
        let name = cstring(name)?;
        // SAFETY: parent is a live directory descriptor and name is NUL-terminated.
        if unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::AlreadyExists {
                return Err(io_error(error));
            }
        } else {
            parent.sync_all().map_err(io_error)?;
        }
    }
    open_at(parent, name, libc::O_RDONLY | libc::O_DIRECTORY, 0)
        .map_err(|_| rejected("attachment directory is missing or linked"))
}

fn open_at(parent: &File, name: &str, flags: i32, mode: libc::mode_t) -> std::io::Result<File> {
    let name = CString::new(name).map_err(std::io::Error::other)?;
    // SAFETY: all paths are one validated component beneath an owned descriptor.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            libc::c_uint::from(mode),
        )
    };
    if fd == -1 {
        Err(std::io::Error::last_os_error())
    } else {
        // SAFETY: openat returned a new owned descriptor, transferred exactly once.
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}

fn regular(parent: &File, name: &str) -> Result<File> {
    let file =
        open_at(parent, name, libc::O_RDONLY | libc::O_NONBLOCK, 0).map_err(|_| conflict())?;
    let metadata = file.metadata().map_err(io_error)?;
    if !metadata.is_file() || metadata.nlink() != 1 {
        return Err(conflict());
    }
    Ok(file)
}

fn write_new(parent: &File, name: &str, content: &[u8]) -> Result<()> {
    let mut file = open_at(
        parent,
        name,
        libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
        0o600,
    )
    .map_err(io_error)?;
    file.write_all(content).map_err(io_error)?;
    file.sync_all().map_err(io_error)
}

fn publish_directory(parent: &File, from: &str, to: &str) -> std::io::Result<()> {
    let from = CString::new(from).map_err(std::io::Error::other)?;
    let to = CString::new(to).map_err(std::io::Error::other)?;
    #[cfg(target_os = "linux")]
    // SAFETY: both path components and the directory descriptors remain live.
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            parent.as_raw_fd(),
            from.as_ptr(),
            parent.as_raw_fd(),
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    #[cfg(target_os = "macos")]
    // SAFETY: same descriptor/path guarantees; RENAME_EXCL never replaces a target.
    let result = unsafe {
        libc::renameatx_np(
            parent.as_raw_fd(),
            from.as_ptr(),
            parent.as_raw_fd(),
            to.as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

struct Stage {
    parent: File,
    directory: File,
    name: String,
    payload: String,
}
impl Stage {
    fn new(parent: &File, payload: &str) -> Result<Self> {
        let name = format!(".upload-{}", uuid::Uuid::new_v4().simple());
        let encoded = cstring(&name)?;
        // SAFETY: fresh random, single component relative to the retained directory.
        if unsafe { libc::mkdirat(parent.as_raw_fd(), encoded.as_ptr(), 0o700) } != 0 {
            return Err(io_error(std::io::Error::last_os_error()));
        }
        Ok(Self {
            parent: parent.try_clone().map_err(io_error)?,
            directory: child(parent, &name, false)?,
            name,
            payload: payload.into(),
        })
    }
}
impl Drop for Stage {
    fn drop(&mut self) {
        // Published staging names are gone. Never remove through the destination
        // descriptor: it now contains the owner's committed upload.
        let Ok(named) = open_at(
            &self.parent,
            &self.name,
            libc::O_RDONLY | libc::O_DIRECTORY,
            0,
        ) else {
            return;
        };
        let Ok(named) = named.metadata() else {
            return;
        };
        let Ok(owned) = self.directory.metadata() else {
            return;
        };
        if named.dev() != owned.dev() || named.ino() != owned.ino() {
            return;
        }
        for file in [&self.payload, RECORD] {
            if let Ok(file) = CString::new(file.as_bytes()) {
                // SAFETY: unlinkat removes only this stage's named directory entry.
                unsafe {
                    libc::unlinkat(self.directory.as_raw_fd(), file.as_ptr(), 0);
                }
            }
        }
        if let Ok(name) = CString::new(self.name.as_bytes()) {
            // SAFETY: removes only the now-empty, unpublished temporary directory.
            unsafe {
                libc::unlinkat(self.parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR);
            }
        }
    }
}
fn cstring(value: &str) -> Result<CString> {
    CString::new(value).map_err(io_error)
}
fn rejected(detail: &str) -> RuntimeError {
    RuntimeError::rejected("attachment_upload_refused", detail)
}
fn conflict() -> RuntimeError {
    RuntimeError::rejected(
        "attachment_upload_conflict",
        "attachment ID already names different or modified content",
    )
}
fn io_error(error: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::unavailable(
        "attachment_upload_io",
        format!("attachment storage failed: {error}"),
    )
}

#[cfg(test)]
#[path = "agent_attachment_tests.rs"]
mod tests;
