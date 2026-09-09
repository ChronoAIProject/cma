use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use crate::failure::{Result, RuntimeError};

pub(crate) fn replace_home(home: &Path, staged: &Path, scratch: &Path) -> Result<()> {
    // The supervisor holds its operation gate and all home writers throughout
    // replacement. Files can share storage with the temporary rollback/staging
    // trees until those trees are removed, before writers are resumed.
    validate_home(home)?;
    let backup = scratch.join(format!(
        ".restore-backup-{}-{}",
        std::process::id(),
        stamp()
    ));
    let mut cleanup = Cleanup(Some(backup.clone()));
    copy_tree(home, &backup)?;
    if let Err(error) = clear_directory(home).and_then(|_| copy_contents(staged, home)) {
        let rollback = clear_directory(home).and_then(|_| copy_contents(&backup, home));
        return match rollback {
            Ok(()) => Err(error),
            Err(rollback) => {
                cleanup.disarm();
                Err(RuntimeError::unavailable(
                    "restore_rollback_failed",
                    format!(
                        "restore failed ({error}); rollback also failed ({rollback}); backup retained at {}",
                        backup.display()
                    ),
                ))
            }
        };
    }
    cleanup
        .remove_now()
        .map_err(copy_io("remove restore backup"))?;
    Ok(())
}

fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    crate::fs_durability::create_dir(destination).map_err(copy_io("create restore backup"))?;
    copy_contents(source, destination)
}

fn copy_contents(source: &Path, destination: &Path) -> Result<()> {
    let mut directories = Vec::new();
    let mut pending = vec![(source.to_path_buf(), destination.to_path_buf())];
    while let Some((from, to)) = pending.pop() {
        let metadata =
            std::fs::symlink_metadata(&from).map_err(copy_io("inspect copied directory"))?;
        directories.push((to.clone(), mode(&metadata)));
        let entries = std::fs::read_dir(&from)
            .map_err(copy_io("list copied directory"))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(copy_io("read copied directory entry"))?;
        for entry in entries {
            let source_path = entry.path();
            let target_path = to.join(entry.file_name());
            let metadata =
                std::fs::symlink_metadata(&source_path).map_err(copy_io("inspect copied entry"))?;
            let kind = metadata.file_type();
            if kind.is_dir() {
                crate::fs_durability::create_dir(&target_path)
                    .map_err(copy_io("create copied directory"))?;
                pending.push((source_path, target_path));
            } else if kind.is_file() {
                copy_file(&source_path, &target_path, mode(&metadata))?;
            } else if kind.is_symlink() {
                let target =
                    std::fs::read_link(&source_path).map_err(copy_io("read copied symlink"))?;
                crate::fs_durability::symlink(&target, &target_path)
                    .map_err(copy_io("copy restored symlink"))?;
            } else {
                return Err(RuntimeError::rejected(
                    "restore_entry_unsupported",
                    "home contains an unsupported filesystem entry",
                ));
            }
        }
    }
    directories.sort_by_key(|(path, _)| std::cmp::Reverse(path.components().count()));
    for (path, mode) in directories {
        sync_directory_mode(&path, mode)?;
    }
    Ok(())
}

fn copy_file(source: &Path, destination: &Path, mode: u32) -> Result<()> {
    let mut opened = open_source(source).map_err(copy_io("open copied file"))?;
    let metadata = opened
        .metadata()
        .map_err(copy_io("inspect opened copied file"))?;
    if !metadata.is_file() {
        return Err(RuntimeError::rejected(
            "restore_entry_unsupported",
            "home file changed to an unsupported filesystem entry",
        ));
    }
    if link_quiet_file(source, destination, &opened, &metadata, mode)? {
        return Ok(());
    }
    let mut output = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)
        .map_err(copy_io("create copied file"))?;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = opened
            .read(&mut buffer)
            .map_err(copy_io("read copied file"))?;
        if read == 0 {
            break;
        }
        output
            .write_all(&buffer[..read])
            .map_err(copy_io("write copied file"))?;
    }
    set_mode(&output, mode)?;
    output.sync_all().map_err(copy_io("sync copied file"))?;
    crate::fs_durability::sync_parent(destination).map_err(copy_io("sync copied file parent"))
}

#[cfg(unix)]
fn link_quiet_file(
    source: &Path,
    destination: &Path,
    opened: &std::fs::File,
    metadata: &std::fs::Metadata,
    mode: u32,
) -> Result<bool> {
    use std::os::unix::fs::MetadataExt as _;

    // Copy when sanitising special permission bits, without changing the
    // source inode. Mounts that cannot link retain the streaming copy path.
    if metadata.mode() & 0o7777 != mode {
        return Ok(false);
    }
    match std::fs::hard_link(source, destination) {
        Ok(()) => {}
        Err(error)
            if matches!(
                error.raw_os_error(),
                Some(libc::EXDEV | libc::EOPNOTSUPP | libc::EMLINK)
            ) =>
        {
            return Ok(false);
        }
        Err(error) => return Err(copy_io("link restored file")(error)),
    }
    let linked =
        std::fs::symlink_metadata(destination).map_err(copy_io("inspect linked restored file"))?;
    if !linked.is_file()
        || linked.file_type().is_symlink()
        || linked.dev() != metadata.dev()
        || linked.ino() != metadata.ino()
    {
        crate::fs_durability::remove_file(destination)
            .map_err(copy_io("remove changed restore source link"))?;
        return Err(RuntimeError::rejected(
            "restore_entry_changed",
            "restore source changed while linking its file",
        ));
    }
    opened
        .sync_all()
        .map_err(copy_io("sync linked restored file"))?;
    crate::fs_durability::sync_parent(destination)
        .map_err(copy_io("sync linked restored file parent"))?;
    Ok(true)
}

#[cfg(not(unix))]
fn link_quiet_file(
    _source: &Path,
    _destination: &Path,
    _opened: &std::fs::File,
    _metadata: &std::fs::Metadata,
    _mode: u32,
) -> Result<bool> {
    Ok(false)
}

fn open_source(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    options.open(path)
}

fn sync_directory_mode(path: &Path, mode: u32) -> Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW);
    }
    let directory = options
        .open(path)
        .map_err(copy_io("open copied directory"))?;
    if !directory
        .metadata()
        .map_err(copy_io("inspect opened copied directory"))?
        .is_dir()
    {
        return Err(RuntimeError::rejected(
            "restore_entry_unsupported",
            "home directory changed to an unsupported filesystem entry",
        ));
    }
    directory
        .sync_all()
        .map_err(copy_io("sync copied directory"))?;
    set_mode(&directory, mode)?;
    directory
        .sync_all()
        .map_err(copy_io("sync copied directory mode"))
}

fn clear_directory(root: &Path) -> Result<()> {
    for entry in std::fs::read_dir(root).map_err(copy_io("list home for replacement"))? {
        let entry = entry.map_err(copy_io("read home entry for replacement"))?;
        let path = entry.path();
        let kind = entry
            .file_type()
            .map_err(copy_io("inspect home entry for replacement"))?;
        if kind.is_dir() && !kind.is_symlink() {
            remove_tree(&path).map_err(copy_io("remove old home directory"))?;
        } else {
            crate::fs_durability::remove_file(&path).map_err(copy_io("remove old home entry"))?;
        }
    }
    Ok(())
}

fn validate_home(home: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(home).map_err(copy_io("inspect restore home"))?;
    if home == Path::new("/")
        || !home.is_absolute()
        || !metadata.is_dir()
        || metadata.file_type().is_symlink()
    {
        return Err(RuntimeError::rejected(
            "restore_home_invalid",
            "restore home is not a safe absolute directory",
        ));
    }
    Ok(())
}

/// Remove a supervisor-owned tree, including archived read-only directories.
/// Only directory modes change: regular files may share inodes with live home.
pub(crate) fn remove_tree(root: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return crate::fs_durability::sync_parent(root);
        }
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => {
            return Err(std::io::Error::other(
                "removal root is not a real directory",
            ))
        }
        Err(error) => return Err(error),
    }
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW);
        }
        let directory = options.open(&path)?;
        let metadata = directory.metadata()?;
        if !metadata.is_dir() {
            return Err(std::io::Error::other(
                "removal entry changed from a directory",
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let current = mode(&metadata);
            if current & 0o700 != 0o700 {
                directory.set_permissions(std::fs::Permissions::from_mode(current | 0o700))?;
            }
        }
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                pending.push(entry.path());
            }
        }
    }
    crate::fs_durability::remove_dir_all(root)
}

#[cfg(unix)]
fn mode(metadata: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::MetadataExt as _;
    metadata.mode() & 0o777
}

#[cfg(not(unix))]
fn mode(_metadata: &std::fs::Metadata) -> u32 {
    0o755
}

#[cfg(unix)]
fn set_mode(file: &std::fs::File, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    file.set_permissions(std::fs::Permissions::from_mode(mode))
        .map_err(copy_io("set copied mode"))
}

#[cfg(not(unix))]
fn set_mode(_file: &std::fs::File, _mode: u32) -> Result<()> {
    Ok(())
}

fn copy_io(operation: &'static str) -> impl FnOnce(std::io::Error) -> RuntimeError + Copy {
    move |error| RuntimeError::unavailable("restore_io_failed", format!("{operation}: {error}"))
}

fn stamp() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

struct Cleanup(Option<PathBuf>);

impl Cleanup {
    fn disarm(&mut self) {
        self.0 = None;
    }

    fn remove_now(&mut self) -> std::io::Result<()> {
        let Some(path) = self.0.as_ref() else {
            return Ok(());
        };
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                remove_tree(path)?;
            }
            Ok(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "restore backup is not a real directory",
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                crate::fs_durability::sync_parent(path)?;
            }
            Err(error) => return Err(error),
        }
        self.0 = None;
        Ok(())
    }
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = self.remove_now();
    }
}

#[cfg(all(test, unix))]
#[path = "checkpoint_copy_tests.rs"]
mod tests;
