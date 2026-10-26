//! Initialize an empty, controller-owned volume before any runtime can use it.
//! The init container mounts the volume root; the runtime only mounts its three
//! subdirectories. Completion is durable and retries never overwrite user data.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::path::Path;

use crate::fs_durability as durable;

const COMPLETE: &str = ".cma-home-layout-v1";
const STAGING: &str = ".cma-home-initializing-v1";
const COMPLETE_V2: &str = ".cma-home-layout-v2";
const STAGING_V2: &str = ".cma-home-initializing-v2";
const DIRECTORIES: [&str; 3] = ["home", "state", "transfer"];

pub(crate) fn initialize(root: &Path, image_home: &Path) -> io::Result<()> {
    initialize_layout(root, image_home, false)
}

/// V2 persists all of /home; the public user seed still belongs at /home/cma.
pub(crate) fn initialize_home_tree(root: &Path, image_home: &Path) -> io::Result<()> {
    initialize_layout(root, image_home, true)
}

fn initialize_layout(root: &Path, image_home: &Path, full_home: bool) -> io::Result<()> {
    let (complete_name, staging_name) = if full_home {
        (COMPLETE_V2, STAGING_V2)
    } else {
        (COMPLETE, STAGING)
    };
    validate_root(root)?;
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name();
        if !DIRECTORIES.iter().any(|allowed| name == *allowed)
            && name != complete_name
            && name != staging_name
            && name != "lost+found"
        {
            return Err(invalid("volume contains an unknown layout entry"));
        }
        directory(&entry.path())?;
    }
    let complete = root.join(complete_name);
    if exists(&complete)? {
        ensure_empty(&complete)?;
        if exists(&root.join(staging_name))? {
            return Err(invalid(
                "completed volume still contains initialization staging",
            ));
        }
        for name in DIRECTORIES {
            owned_directory(&root.join(name))?;
        }
        if full_home {
            owned_directory(&root.join("home/cma"))?;
        }
        // Do not inspect, seed, chmod or replace the owner's retained contents.
        return Ok(());
    }

    directory(image_home)?;
    for name in ["state", "transfer"] {
        let path = root.join(name);
        if exists(&path)? {
            owned_directory(&path)?;
            ensure_empty(&path)?;
        }
    }
    let home = root.join("home");
    let staging = root.join(staging_name);
    if exists(&home)? {
        owned_directory(&home)?;
        if exists(&staging)? || !same_seed(image_home, &home, full_home)? {
            return Err(invalid("uninitialized home differs from the public image"));
        }
    } else {
        if exists(&staging)? {
            owned_directory(&staging)?;
            // An init-container restart may leave an unpublished partial copy.
            // No runtime can start before the completion marker is published.
            durable::remove_dir_all(&staging)?;
        }
        private_directory(&staging)?;
        if full_home {
            private_directory(&staging.join("cma"))?;
            private_directory(&staging.join(".codex"))?;
            copy_tree(image_home, &staging.join("cma"))?;
        } else {
            copy_tree(image_home, &staging)?;
        }
        durable::sync_tree(&staging)?;
        durable::rename(&staging, &home)?;
    }
    for name in ["state", "transfer"] {
        let path = root.join(name);
        if !exists(&path)? {
            private_directory(&path)?;
        }
    }
    // mkdir is the atomic commit; every required directory and copied file was
    // synced first. No credentials, network, Kubernetes client or shell needed.
    private_directory(&complete)
}

fn same_seed(image_home: &Path, home: &Path, full_home: bool) -> io::Result<bool> {
    if !full_home {
        return same_tree(image_home, home);
    }
    if fs::read_dir(home)?.count() != 2 {
        return Ok(false);
    }
    owned_directory(&home.join("cma"))?;
    owned_directory(&home.join(".codex"))?;
    ensure_empty(&home.join(".codex"))?;
    same_tree(image_home, &home.join("cma"))
}

fn validate_root(root: &Path) -> io::Result<()> {
    if !root.is_absolute()
        || root == Path::new("/")
        || root.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
    {
        return Err(invalid("volume root must be a non-root absolute directory"));
    }
    for ancestor in root.ancestors() {
        directory(ancestor)?;
    }
    Ok(())
}

fn copy_tree(source: &Path, target: &Path) -> io::Result<()> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source = entry.path();
        let target = target.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            private_directory(&target)?;
            copy_tree(&source, &target)?;
        } else if kind.is_file() {
            let mut input = regular_file(&source)?;
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target)?;
            io::copy(&mut input, &mut output)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                output.set_permissions(fs::Permissions::from_mode(
                    input.metadata()?.permissions().mode() & 0o777,
                ))?;
            }
            output.flush()?;
            output.sync_all()?;
        } else {
            return Err(invalid(
                "public image home contains a symlink or special file",
            ));
        }
    }
    Ok(())
}

fn same_tree(source: &Path, target: &Path) -> io::Result<bool> {
    directory(source)?;
    directory(target)?;
    if fs::read_dir(source)?.count() != fs::read_dir(target)?.count() {
        return Ok(false);
    }
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let other = target.join(entry.file_name());
        if !exists(&other)? {
            return Ok(false);
        }
        let kind = entry.file_type()?;
        if kind.is_dir() {
            if !same_tree(&entry.path(), &other)? {
                return Ok(false);
            }
        } else if kind.is_file() {
            let mut left = regular_file(&entry.path())?;
            let mut right = regular_file(&other)?;
            if left.metadata()?.len() != right.metadata()?.len() {
                return Ok(false);
            }
            let mut a = [0_u8; 8192];
            let mut b = [0_u8; 8192];
            loop {
                let size = left.read(&mut a)?;
                if size == 0 {
                    break;
                }
                right.read_exact(&mut b[..size])?;
                if a[..size] != b[..size] {
                    return Ok(false);
                }
            }
        } else {
            return Err(invalid(
                "public image home contains a symlink or special file",
            ));
        }
    }
    Ok(true)
}

fn regular_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() || fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(invalid("initialization source is not a regular file"));
    }
    Ok(file)
}

fn directory(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(invalid("home layout entry is not a real directory"));
    }
    Ok(())
}

fn owned_directory(path: &Path) -> io::Result<()> {
    directory(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        // The template runs this init container and the runtime as UID 10001.
        if fs::symlink_metadata(path)?.uid() != unsafe { libc::geteuid() } {
            return Err(invalid("home layout directory belongs to another user"));
        }
    }
    Ok(())
}

fn private_directory(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder.create(path)?;
    durable::sync_directory(path)?;
    durable::sync_parent(path)
}

fn ensure_empty(path: &Path) -> io::Result<()> {
    if fs::read_dir(path)?.next().is_some() {
        return Err(invalid(
            "initialization directory is unexpectedly non-empty",
        ));
    }
    Ok(())
}

fn exists(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
#[path = "home_initializer_tests.rs"]
mod tests;
