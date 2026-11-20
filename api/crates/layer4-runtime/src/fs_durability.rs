use std::fs::{File, OpenOptions};
use std::io::{self, Write as _};
use std::path::Path;

/// Create every missing directory and persist each new parent entry.
pub(crate) fn create_dir_all(path: &Path) -> io::Result<()> {
    create_dir_all_with(path, &mut sync_directory)
}

fn create_dir_all_with(
    path: &Path,
    sync: &mut impl FnMut(&Path) -> io::Result<()>,
) -> io::Result<()> {
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
        sync(parent(created)?)?;
    }
    Ok(())
}

pub(crate) fn create_dir(path: &Path) -> io::Result<()> {
    std::fs::create_dir(path)?;
    sync_parent(path)
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

pub(crate) fn remove_dir(path: &Path) -> io::Result<()> {
    std::fs::remove_dir(path)?;
    sync_parent(path)
}

pub(crate) fn hard_link(source: &Path, destination: &Path) -> io::Result<()> {
    std::fs::hard_link(source, destination)?;
    sync_parent(destination)
}

#[cfg(unix)]
pub(crate) fn symlink(target: &Path, destination: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, destination)?;
    sync_parent(destination)
}

#[cfg(not(unix))]
pub(crate) fn symlink(_target: &Path, _destination: &Path) -> io::Result<()> {
    Err(io::Error::other("durable symlinks require Unix"))
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

pub(crate) fn sync_file(path: &Path) -> io::Result<()> {
    sync_file_with(path, &mut File::sync_all)
}

pub(crate) fn sync_file_with(
    path: &Path,
    sync: &mut impl FnMut(&File) -> io::Result<()>,
) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "file sync target is not a regular file",
        ));
    }
    sync(&file)
}

/// Persist a complete regular-file/directory tree and its entry in its parent.
pub(crate) fn sync_tree(root: &Path) -> io::Result<()> {
    let metadata = std::fs::symlink_metadata(root)?;
    if metadata.is_file() && !metadata.file_type().is_symlink() {
        sync_file(root)?;
        return sync_parent(root);
    }
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "tree sync root is not a real directory",
        ));
    }
    let mut pending = vec![root.to_path_buf()];
    let mut directories = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() && !kind.is_symlink() {
                pending.push(entry.path());
            } else if kind.is_file() && !kind.is_symlink() {
                sync_file(&entry.path())?;
            } else if !kind.is_symlink() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "tree contains an unsupported filesystem entry",
                ));
            }
        }
        directories.push(directory);
    }
    directories.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for directory in directories {
        sync_directory(&directory)?;
    }
    sync_parent(root)
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

/// Persist every ancestor entry, including those left visible by a failed mkdir
/// sync. No nearer ancestor has a durability guarantee, so continue to root.
pub(crate) fn sync_directory_chain(
    path: &Path,
    sync: &mut impl FnMut(&Path) -> io::Result<()>,
) -> io::Result<()> {
    sync(path)?;
    let mut synced = std::collections::BTreeSet::from([path.canonicalize()?]);
    // Temporary roots can contain aliases (for example the platform temp dir).
    // Persist both the resolved directories and the parents holding each alias.
    let mut pending = vec![std::path::absolute(path)?];
    let mut visited = std::collections::BTreeSet::new();
    while let Some(path) = pending.pop() {
        for ancestor in path.ancestors() {
            if !visited.insert(ancestor.to_path_buf()) {
                continue;
            }
            let resolved = ancestor.canonicalize()?;
            for directory in resolved.ancestors() {
                if synced.insert(directory.to_path_buf()) {
                    sync(directory)?;
                }
            }
            if std::fs::symlink_metadata(ancestor)?
                .file_type()
                .is_symlink()
            {
                let target = std::fs::read_link(ancestor)?;
                pending.push(if target.is_absolute() {
                    target
                } else {
                    parent(ancestor)?.join(target)
                });
            }
        }
    }
    Ok(())
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

#[cfg(test)]
#[path = "fs_durability_tests.rs"]
pub(crate) mod tests;
