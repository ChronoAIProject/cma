//! Prove separation using live directory identities before any output creation.
use super::{changed, invalid_guard, invalid_io, io::Directory};
#[cfg(any(target_os = "linux", test))]
#[path = "isolation_mounts.rs"]
mod mounts;
use crate::failure::Result;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

pub(super) struct Proof {
    roots: Vec<(PathBuf, (u64, u64))>,
}
impl Proof {
    pub(super) fn new(roots: &[&Path]) -> Result<Self> {
        let roots = roots
            .iter()
            .map(|path| {
                let directory = Directory::open(path)?;
                Ok((path.to_path_buf(), identity(&directory)?))
            })
            .collect::<Result<_>>()?;
        Ok(Self { roots })
    }
    pub(super) fn parent(&self, destination: &Path) -> Result<Directory> {
        let parent_path = destination
            .parent()
            .ok_or_else(|| invalid_guard("isolation.parent.present"))?;
        let parent = Directory::open(parent_path)?;
        let ancestors = lineage(&parent)?;
        for (path, expected) in &self.roots {
            let directory = Directory::open(path)?;
            if identity(&directory)? != *expected {
                return Err(changed());
            }
            if destination.starts_with(path)
                || path.starts_with(destination)
                || ancestors.contains(expected)
            {
                return Err(invalid_guard("isolation.parent.no_protected_overlap"));
            }
        }
        #[cfg(target_os = "linux")]
        self.linux_mounts(&parent)?;
        Ok(parent)
    }
}
fn identity(directory: &Directory) -> Result<(u64, u64)> {
    let metadata = directory
        .0
        .metadata()
        .map_err(|e| invalid_io("isolation.identity.fstat", e))?;
    Ok((metadata.dev(), metadata.ino()))
}
fn lineage(directory: &Directory) -> Result<Vec<(u64, u64)>> {
    let mut result = Vec::new();
    let mut directory = Directory(
        directory
            .0
            .try_clone()
            .map_err(|e| invalid_io("isolation.lineage.clone", e))?,
    );
    loop {
        if result.len() == 256 {
            return Err(invalid_guard("isolation.lineage.depth_bound"));
        }
        let current = identity(&directory)?;
        if result.last() == Some(&current) {
            return Ok(result);
        }
        if result.contains(&current) {
            return Err(invalid_guard("isolation.lineage.no_cycle"));
        }
        result.push(current);
        // Walk from the exact retained destination descriptor, never reopen a
        // pathname that may now name a different directory after a rename.
        let fd = unsafe {
            libc::openat(
                directory.0.as_raw_fd(),
                c"..".as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(invalid_io(
                "isolation.lineage.openat",
                std::io::Error::last_os_error(),
            ));
        }
        directory = Directory(unsafe { std::fs::File::from_raw_fd(fd) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn physical_lineage_follows_retained_descriptor_after_parent_replacement() {
        let root = std::env::temp_dir().join(format!("ledger-lineage-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let root = std::fs::canonicalize(root).unwrap();
        let parent = root.join("output-parent");
        let protected = root.join("protected");
        std::fs::create_dir_all(&parent).unwrap();
        std::fs::create_dir(&protected).unwrap();
        let retained = Directory::open(&parent).unwrap();
        std::fs::rename(&parent, protected.join("moved-parent")).unwrap();
        std::fs::create_dir(&parent).unwrap();
        let protected_id = identity(&Directory::open(&protected).unwrap()).unwrap();
        assert!(lineage(&retained).unwrap().contains(&protected_id));
        assert!(!lineage(&Directory::open(&parent).unwrap())
            .unwrap()
            .contains(&protected_id));
        std::fs::remove_dir_all(root).unwrap();
    }
}
