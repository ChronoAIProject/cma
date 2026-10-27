//! Descriptor-relative filesystem operations confined to the retained `/home`.
//! No filesystem path from the caller is ever passed to an absolute-path open.
use crate::{
    config::RuntimeConfig,
    failure::{Result, RuntimeError},
};
use base64::Engine as _;
use layer3_adapter::{
    AgentFileDeleteReceipt, AgentFileDeleteRequest, AgentFileEntry, AgentFileKind,
    AgentFileListReceipt, AgentFileListRequest, AgentFileReadReceipt, AgentFileReadRequest,
    AgentFileWriteReceipt, AgentFileWriteRequest, MAX_AGENT_FILE_BYTES, MAX_AGENT_FILE_ENTRIES,
};
use sha2::{Digest as _, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::Write as _,
    os::{fd::AsRawFd as _, unix::fs::OpenOptionsExt as _},
    path::Path,
};

#[path = "agent_filesystem/io.rs"]
mod io;
use io::{cstring, descend, mkdir, names, open_at, parent, read_regular, rename, stat, unlink};

pub(crate) fn list(
    config: &RuntimeConfig,
    request: AgentFileListRequest,
) -> Result<AgentFileListReceipt> {
    identity(
        config,
        &request.sandbox_id,
        request.runtime_generation,
        &request.boot_id,
    )?;
    check_scope(
        request.owner_access,
        &request.protected_paths,
        &request.path,
        false,
    )?;
    let root = root(config)?;
    let directory = descend(&root, &request.path, false)?;
    let mut entries = Vec::new();
    for name in names(&directory)? {
        let path = if request.path.is_empty() {
            name.clone()
        } else {
            format!("{}/{}", request.path, name)
        };
        if check_scope(request.owner_access, &request.protected_paths, &path, false).is_err() {
            continue;
        }
        let metadata = stat(&directory, &name)?;
        let kind = match metadata.st_mode & libc::S_IFMT {
            libc::S_IFDIR => AgentFileKind::Directory,
            libc::S_IFREG => AgentFileKind::File,
            libc::S_IFLNK => AgentFileKind::Symlink,
            _ => continue,
        };
        entries.push(AgentFileEntry {
            path,
            hidden: name.starts_with('.'),
            name,
            kind,
            size_bytes: u64::try_from(metadata.st_size).unwrap_or(0),
            // Directory listings never hash file bodies; opening a file returns
            // its version. Large home trees remain cheap to browse.
            sha256: None,
        });
    }
    entries.sort_by(|a, b| {
        let directory_rank = |kind: AgentFileKind| {
            if kind == AgentFileKind::Directory {
                0
            } else {
                1
            }
        };
        (directory_rank(a.kind), a.name.to_lowercase())
            .cmp(&(directory_rank(b.kind), b.name.to_lowercase()))
    });
    Ok(AgentFileListReceipt {
        path: request.path,
        entries,
    })
}

pub(crate) fn read(
    config: &RuntimeConfig,
    request: AgentFileReadRequest,
) -> Result<AgentFileReadReceipt> {
    identity(
        config,
        &request.sandbox_id,
        request.runtime_generation,
        &request.boot_id,
    )?;
    if request.max_bytes == 0 || request.max_bytes > MAX_AGENT_FILE_BYTES {
        return Err(rejected("file read bound is invalid"));
    }
    check_scope(
        request.owner_access,
        &request.protected_paths,
        &request.path,
        false,
    )?;
    let root = root(config)?;
    let (parent, name) = parent(&root, &request.path, false)?;
    let bytes = read_regular(&parent, &name, request.max_bytes)?;
    Ok(AgentFileReadReceipt {
        path: request.path,
        size_bytes: bytes.len() as u64,
        sha256: digest(&bytes),
        content_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}

pub(crate) fn write(
    config: &RuntimeConfig,
    request: AgentFileWriteRequest,
) -> Result<AgentFileWriteReceipt> {
    identity(
        config,
        &request.sandbox_id,
        request.runtime_generation,
        &request.boot_id,
    )?;
    let bytes = request
        .content()
        .map_err(|_| rejected("file content is invalid or too large"))?;
    check_scope(
        request.owner_access,
        &request.protected_paths,
        &request.path,
        false,
    )?;
    let root = root(config)?;
    let (parent, name) = parent(&root, &request.path, true)?;
    if request.directory {
        if !bytes.is_empty() || request.expected_sha256.is_some() {
            return Err(rejected("directory creation cannot include file content"));
        }
        mkdir(&parent, &name)?;
        let _directory = open_at(&parent, &name, libc::O_RDONLY | libc::O_DIRECTORY, 0)?;
        return Ok(AgentFileWriteReceipt {
            path: request.path,
            size_bytes: 0,
            sha256: digest(&[]),
        });
    }
    let previous = match stat(&parent, &name) {
        Ok(metadata) => {
            if metadata.st_mode & libc::S_IFMT != libc::S_IFREG || metadata.st_nlink != 1 {
                return Err(rejected("target is not an single-link regular file"));
            }
            // Updating an existing file requires its version. This also prevents
            // a delayed create/upload from replacing somebody else's new file.
            let current = digest(&read_regular(&parent, &name, MAX_AGENT_FILE_BYTES)?);
            if current == digest(&bytes) {
                return Ok(AgentFileWriteReceipt {
                    path: request.path,
                    size_bytes: bytes.len() as u64,
                    sha256: current,
                });
            }
            let expected = request.expected_sha256.as_ref().ok_or_else(conflict)?;
            if current != *expected {
                return Err(conflict());
            }
            Some(metadata)
        }
        Err(error) if error.code == "file_not_found" => {
            if request.expected_sha256.is_some() {
                return Err(conflict());
            }
            None
        }
        Err(error) => return Err(error),
    };
    let stage_name = format!(".cma-write-{}", uuid::Uuid::new_v4().simple());
    let mode = previous
        .as_ref()
        .map_or(0o600, |metadata| metadata.st_mode & 0o777);
    let mut stage = open_at(
        &parent,
        &stage_name,
        libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
        mode,
    )?;
    let result = (|| {
        stage.write_all(&bytes).map_err(io_error)?;
        stage.sync_all().map_err(io_error)?;
        if let Some(original) = previous {
            let current = stat(&parent, &name)?;
            if current.st_ino != original.st_ino
                || current.st_dev != original.st_dev
                || digest(&read_regular(&parent, &name, MAX_AGENT_FILE_BYTES)?)
                    != *request.expected_sha256.as_ref().expect("existing version")
            {
                return Err(conflict());
            }
            rename(&parent, &stage_name, &name)?;
        } else {
            // linkat publishes exclusively, so a concurrent create never gets overwritten.
            let source = cstring(&stage_name)?;
            let destination = cstring(&name)?;
            // SAFETY: both names are single components and the directory FD stays live.
            if unsafe {
                libc::linkat(
                    parent.as_raw_fd(),
                    source.as_ptr(),
                    parent.as_raw_fd(),
                    destination.as_ptr(),
                    0,
                )
            } != 0
            {
                return Err(conflict());
            }
            unlink(&parent, &stage_name, false)?;
        }
        parent.sync_all().map_err(io_error)?;
        Ok(AgentFileWriteReceipt {
            path: request.path,
            size_bytes: bytes.len() as u64,
            sha256: digest(&bytes),
        })
    })();
    if result.is_err() {
        let _ = unlink(&parent, &stage_name, false);
    }
    result
}

pub(crate) fn delete(
    config: &RuntimeConfig,
    request: AgentFileDeleteRequest,
) -> Result<AgentFileDeleteReceipt> {
    identity(
        config,
        &request.sandbox_id,
        request.runtime_generation,
        &request.boot_id,
    )?;
    check_scope(
        request.owner_access,
        &request.protected_paths,
        &request.path,
        true,
    )?;
    let root = root(config)?;
    let (parent, name) = parent(&root, &request.path, false)?;
    let mut remaining = MAX_AGENT_FILE_ENTRIES;
    remove(&parent, &name, request.recursive, &mut remaining, 0)?;
    parent.sync_all().map_err(io_error)?;
    Ok(AgentFileDeleteReceipt {
        path: request.path,
        deleted: true,
    })
}

fn check_scope(owner: bool, provider_paths: &[String], path: &str, remove: bool) -> Result<()> {
    if owner {
        return Ok(());
    }
    // These are supervisor-owned state and credential custody. A share of an
    // Agent never grants access to the runtime's control plane or other Agents.
    for protected in [".cma", "cma/.cma"]
        .into_iter()
        .chain(provider_paths.iter().map(String::as_str))
    {
        if !layer3_adapter::valid_agent_file_path(protected)
            || protected.is_empty()
            || path == protected
            || path.starts_with(&format!("{protected}/"))
            || (remove && (path.is_empty() || protected.starts_with(&format!("{path}/"))))
        {
            return Err(rejected(
                "runtime credentials and control files are not shared",
            ));
        }
    }
    Ok(())
}

fn remove(
    parent: &File,
    name: &str,
    recursive: bool,
    remaining: &mut usize,
    depth: usize,
) -> Result<()> {
    if *remaining == 0 || depth > 64 {
        return Err(rejected(
            "directory deletion exceeds its entry or depth limit",
        ));
    }
    *remaining -= 1;
    let metadata = stat(parent, name)?;
    if metadata.st_mode & libc::S_IFMT == libc::S_IFDIR {
        let directory = open_at(parent, name, libc::O_RDONLY | libc::O_DIRECTORY, 0)?;
        if recursive {
            for child in names(&directory)? {
                remove(&directory, &child, true, remaining, depth + 1)?;
            }
        }
        let current = stat(parent, name)?;
        if current.st_ino != metadata.st_ino || current.st_dev != metadata.st_dev {
            return Err(conflict());
        }
        unlink(parent, name, true)
    } else {
        unlink(parent, name, false)
    }
}

fn root(config: &RuntimeConfig) -> Result<File> {
    let path = if config.home == Path::new("/home/cma") {
        if !crate::agent_attachment::supported(config) {
            return Err(rejected("filesystem requires the complete persistent home"));
        }
        Path::new("/home")
    } else {
        #[cfg(not(test))]
        return Err(rejected("filesystem requires the complete persistent home"));
        #[cfg(test)]
        config.home.as_path()
    };
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(io_error)
}
fn identity(config: &RuntimeConfig, sandbox: &str, generation: u64, boot: &str) -> Result<()> {
    if !layer3_adapter::valid_agent_filesystem_identity(sandbox, generation, boot)
        || config.sandbox_id != sandbox
        || config.generation != generation
    {
        return Err(rejected("filesystem request does not name this runtime"));
    }
    Ok(())
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn io_error(error: std::io::Error) -> RuntimeError {
    match error.kind() {
        std::io::ErrorKind::NotFound => {
            RuntimeError::rejected("file_not_found", "file or directory does not exist")
        }
        std::io::ErrorKind::PermissionDenied => {
            RuntimeError::rejected("filesystem_permission", "file permission denied")
        }
        _ if matches!(error.raw_os_error(), Some(libc::ELOOP | libc::ENOTDIR)) => {
            rejected("symlinks and non-directory parents are not traversable")
        }
        _ => RuntimeError::unavailable("filesystem_io", "filesystem operation failed"),
    }
}
fn rejected(detail: &str) -> RuntimeError {
    RuntimeError::rejected("filesystem_rejected", detail)
}
fn conflict() -> RuntimeError {
    RuntimeError::rejected("file_conflict", "file changed since it was read")
}

#[cfg(test)]
#[path = "agent_filesystem_tests.rs"]
mod tests;
