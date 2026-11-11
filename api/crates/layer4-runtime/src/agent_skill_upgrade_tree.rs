//! Compare all file contents, paths and modes without traversing symbolic links.
use super::{conflict, io, MARKER};
use crate::{agent_filesystem::io as fd, config::RuntimeConfig, failure::Result};
use layer3_adapter::{AgentSkillInstallReceipt, AgentSkillInstallRequest};
use sha2::{Digest as _, Sha256};
use std::{
    ffi::CString,
    fs::{File, OpenOptions},
    io::Read as _,
    os::{
        fd::AsRawFd as _,
        unix::fs::{MetadataExt as _, OpenOptionsExt as _},
    },
    path::Path,
};

fn directory(path: &Path) -> Result<File> {
    // Open every ancestor separately so even an intermediate symlink is refused.
    let mut current = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open("/")
        .map_err(io)?;
    for part in path.components() {
        match part {
            std::path::Component::RootDir => {}
            std::path::Component::Normal(name) => {
                let name = name
                    .to_str()
                    .ok_or_else(|| conflict("non-UTF8 skill path"))?;
                current = fd::open_at(&current, name, libc::O_RDONLY | libc::O_DIRECTORY, 0)
                    .map_err(|_| conflict("skill parent is not a real directory"))?;
            }
            _ => return Err(conflict("noncanonical skill parent")),
        }
    }
    Ok(current)
}

pub(super) fn receipt_matches(path: &Path, expected: &AgentSkillInstallReceipt) -> Result<bool> {
    let directory = directory(path)?;
    match fd::stat(&directory, MARKER) {
        Ok(_) => {
            let bytes = fd::read_regular(&directory, MARKER, 4096)
                .map_err(|_| conflict("invalid installation receipt"))?;
            Ok(serde_json::from_slice::<AgentSkillInstallReceipt>(&bytes)
                .ok()
                .as_ref()
                == Some(expected))
        }
        Err(_) => Ok(false),
    }
}
pub(super) fn digest(path: &Path, receipt: Option<&AgentSkillInstallReceipt>) -> Result<String> {
    let directory = directory(path)?;
    let mut hash = Sha256::new();
    hash.update((directory.metadata().map_err(io)?.mode() & 0o177777).to_be_bytes());
    let mut entries = 0usize;
    let mut bytes = 0u64;
    walk(&directory, "", receipt, &mut hash, &mut entries, &mut bytes)?;
    Ok(format!("{:x}", hash.finalize()))
}
fn walk(
    directory: &File,
    prefix: &str,
    receipt: Option<&AgentSkillInstallReceipt>,
    hash: &mut Sha256,
    entries: &mut usize,
    bytes: &mut u64,
) -> Result<()> {
    let mut names = fd::names(directory).map_err(|_| conflict("unreadable skill directory"))?;
    names.sort();
    if prefix.is_empty() && receipt.is_some() && !names.iter().any(|name| name == MARKER) {
        return Err(conflict("previous installation receipt is absent"));
    }
    for name in names {
        if prefix.is_empty() && name == MARKER {
            let expected = receipt.ok_or_else(|| conflict("unexpected installation receipt"))?;
            let data = fd::read_regular(directory, &name, 4096)
                .map_err(|_| conflict("invalid installation receipt"))?;
            if serde_json::from_slice::<AgentSkillInstallReceipt>(&data)
                .ok()
                .as_ref()
                != Some(expected)
            {
                return Err(conflict(
                    "installation receipt does not match the expected version",
                ));
            }
            continue;
        }
        *entries += 1;
        if *entries > 8192 || prefix.len() > 4096 {
            return Err(conflict("skill tree exceeds comparison bounds"));
        }
        let metadata =
            fd::stat(directory, &name).map_err(|_| conflict("unreadable skill entry"))?;
        let kind = metadata.st_mode & libc::S_IFMT;
        if !matches!(kind, libc::S_IFREG | libc::S_IFDIR) {
            return Err(conflict("skill contains a symlink or special entry"));
        }
        let mut file = fd::open_at(
            directory,
            &name,
            libc::O_RDONLY
                | if kind == libc::S_IFDIR {
                    libc::O_DIRECTORY
                } else {
                    0
                },
            0,
        )
        .map_err(|_| conflict("skill entry changed during comparison"))?;
        let before = file.metadata().map_err(io)?;
        #[cfg(target_os = "macos")]
        let device = metadata.st_dev as u64;
        #[cfg(not(target_os = "macos"))]
        let device = metadata.st_dev;
        if before.ino() != metadata.st_ino || before.dev() != device {
            return Err(conflict("skill entry changed during comparison"));
        }
        let relative = format!("{prefix}{name}");
        hash.update((relative.len() as u64).to_be_bytes());
        hash.update(relative.as_bytes());
        hash.update((before.mode() & 0o177777).to_be_bytes());
        if kind == libc::S_IFDIR {
            walk(&file, &(relative + "/"), None, hash, entries, bytes)?;
        } else {
            if before.nlink() != 1 {
                return Err(conflict("skill contains a hard linked file"));
            }
            *bytes = bytes
                .checked_add(before.len())
                .ok_or_else(|| conflict("skill tree too large"))?;
            if *bytes > 256 * 1024 * 1024 {
                return Err(conflict("skill tree too large"));
            }
            hash.update(before.len().to_be_bytes());
            let mut remaining = before.len();
            let mut buffer = [0u8; 65536];
            while remaining > 0 {
                let count = file
                    .read(&mut buffer[..remaining.min(65536) as usize])
                    .map_err(io)?;
                if count == 0 {
                    return Err(conflict("skill file changed during comparison"));
                }
                remaining -= count as u64;
                hash.update(&buffer[..count]);
            }
            let after = file.metadata().map_err(io)?;
            if before.len() != after.len()
                || before.mtime() != after.mtime()
                || before.mtime_nsec() != after.mtime_nsec()
                || before.ctime() != after.ctime()
                || before.ctime_nsec() != after.ctime_nsec()
            {
                return Err(conflict("skill file changed during comparison"));
            }
        }
    }
    Ok(())
}

pub(super) fn exchange(
    config: &RuntimeConfig,
    request: &AgentSkillInstallRequest,
    stage: &Path,
) -> Result<()> {
    let parent = directory(&crate::agent_skill_install::root(config, request)?)?;
    let staged = directory(stage)?;
    let source = CString::new("new").expect("literal");
    let target =
        CString::new(request.skill.id.as_str()).map_err(|_| conflict("invalid skill id"))?;
    #[cfg(target_os = "linux")]
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            staged.as_raw_fd(),
            source.as_ptr(),
            parent.as_raw_fd(),
            target.as_ptr(),
            libc::RENAME_EXCHANGE,
        )
    };
    #[cfg(target_os = "macos")]
    let result = unsafe {
        libc::renameatx_np(
            staged.as_raw_fd(),
            source.as_ptr(),
            parent.as_raw_fd(),
            target.as_ptr(),
            libc::RENAME_SWAP,
        )
    };
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let result = -1;
    if result != 0 {
        return Err(io(std::io::Error::last_os_error()));
    }
    Ok(())
}
