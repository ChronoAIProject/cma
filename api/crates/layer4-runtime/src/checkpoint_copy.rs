use std::path::{Path, PathBuf};

use crate::failure::{Result, RuntimeError};

pub(crate) fn replace_home(home: &Path, staged: &Path, scratch: &Path) -> Result<()> {
    validate_home(home)?;
    let backup = scratch.join(format!(
        ".restore-backup-{}-{}",
        std::process::id(),
        stamp()
    ));
    let mut cleanup = Cleanup(Some(backup.clone()));
    copy_tree(home, &backup)?;
    clear_directory(home)?;
    if let Err(error) = copy_contents(staged, home) {
        let rollback = clear_directory(home).and_then(|_| copy_contents(&backup, home));
        return match rollback {
            Ok(()) => Err(error),
            Err(rollback) => Err(RuntimeError::unavailable(
                "restore_rollback_failed",
                format!("restore failed ({error}); rollback also failed ({rollback})"),
            )),
        };
    }
    cleanup.remove_now();
    Ok(())
}

fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    std::fs::create_dir(destination).map_err(copy_io("create restore backup"))?;
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
                std::fs::create_dir(&target_path).map_err(copy_io("create copied directory"))?;
                pending.push((source_path, target_path));
            } else if kind.is_file() {
                std::fs::copy(&source_path, &target_path).map_err(copy_io("copy restored file"))?;
                set_mode(&target_path, mode(&metadata))?;
            } else if kind.is_symlink() {
                let target =
                    std::fs::read_link(&source_path).map_err(copy_io("read copied symlink"))?;
                create_symlink(&target, &target_path).map_err(copy_io("copy restored symlink"))?;
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
        set_mode(&path, mode)?;
    }
    Ok(())
}

fn clear_directory(root: &Path) -> Result<()> {
    for entry in std::fs::read_dir(root).map_err(copy_io("list home for replacement"))? {
        let entry = entry.map_err(copy_io("read home entry for replacement"))?;
        let path = entry.path();
        let kind = entry
            .file_type()
            .map_err(copy_io("inspect home entry for replacement"))?;
        if kind.is_dir() && !kind.is_symlink() {
            std::fs::remove_dir_all(path).map_err(copy_io("remove old home directory"))?;
        } else {
            std::fs::remove_file(path).map_err(copy_io("remove old home entry"))?;
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
fn set_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .map_err(copy_io("set copied mode"))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn create_symlink(target: &Path, destination: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, destination)
}

#[cfg(not(unix))]
fn create_symlink(_target: &Path, _destination: &Path) -> std::io::Result<()> {
    Err(std::io::Error::other("restore symlinks require Unix"))
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
    fn remove_now(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        self.remove_now();
    }
}
