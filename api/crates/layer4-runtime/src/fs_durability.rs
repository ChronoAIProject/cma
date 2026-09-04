use std::fs::{File, OpenOptions};
use std::io::{self, Write as _};
use std::path::Path;

/// Create every missing directory and persist each new parent entry.
pub(crate) fn create_dir_all(path: &Path) -> io::Result<()> {
    let mut missing = Vec::new();
    let mut cursor = path;
    while std::fs::symlink_metadata(cursor)
        .is_err_and(|error| error.kind() == io::ErrorKind::NotFound)
    {
        missing.push(cursor.to_path_buf());
        cursor = cursor.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "directory has no parent")
        })?;
    }
    std::fs::create_dir_all(path)?;
    for created in missing.iter().rev() {
        sync_parent(created)?;
    }
    Ok(())
}

/// Rename an entry and persist removal from its old parent and insertion in its new parent.
pub(crate) fn rename(source: &Path, destination: &Path) -> io::Result<()> {
    std::fs::rename(source, destination)?;
    sync_distinct_parents(source, destination)
}

pub(crate) fn remove_file(path: &Path) -> io::Result<()> {
    std::fs::remove_file(path)?;
    sync_parent(path)
}

pub(crate) fn remove_dir_all(path: &Path) -> io::Result<()> {
    std::fs::remove_dir_all(path)?;
    sync_parent(path)
}

pub(crate) fn hard_link(source: &Path, destination: &Path) -> io::Result<()> {
    std::fs::hard_link(source, destination)?;
    sync_parent(destination)
}

pub(crate) fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let existed = std::fs::symlink_metadata(path).is_ok();
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    if existed {
        Ok(())
    } else {
        sync_parent(path)
    }
}

pub(crate) fn sync_parent(path: &Path) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "entry has no parent"))?;
    sync_directory(parent)
}

pub(crate) fn sync_directory(path: &Path) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW);
    }
    let directory: File = options.open(path)?;
    if !directory.metadata()?.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "directory sync target is not a directory",
        ));
    }
    directory.sync_all()
}

fn sync_distinct_parents(left: &Path, right: &Path) -> io::Result<()> {
    let left_parent = parent(left)?;
    let right_parent = parent(right)?;
    let left_result = sync_directory(left_parent);
    let right_result = if left_parent == right_parent {
        Ok(())
    } else {
        sync_directory(right_parent)
    };
    left_result.and(right_result)
}

fn parent(path: &Path) -> io::Result<&Path> {
    path.parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "entry has no parent"))
}
