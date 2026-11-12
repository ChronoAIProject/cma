//! Prove separation using live directory identities before any output creation.
#[cfg(target_os = "linux")]
use super::invalid;
use super::{changed, invalid_guard, invalid_io, io::Directory};
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

    #[cfg(target_os = "linux")]
    fn linux_mounts(&self, parent: &Directory) -> Result<()> {
        use std::io::Read;
        let output = identity(parent)?;
        let output_mount = mount_id(parent)?;
        let mut devices = vec![output.0];
        for (path, id) in &self.roots {
            devices.push(id.0);
            if id.0 == output.0 && mount_id(&Directory::open(path)?)? != output_mount {
                // Different views of one filesystem can hide the physical
                // ancestry of a bind-mounted subtree. Do not infer separation.
                return Err(invalid_guard("isolation.linux_mounts.same_filesystem_view"));
            }
        }
        let mut bytes = Vec::new();
        std::fs::File::open("/proc/self/mountinfo")
            .map_err(|e| invalid_io("isolation.linux_mounts.open_mountinfo", e))?
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| invalid_io("isolation.linux_mounts.read_mountinfo", e))?;
        if bytes.len() > 1024 * 1024 {
            return Err(invalid_guard("isolation.linux_mounts.mountinfo_byte_bound"));
        }
        let text = std::str::from_utf8(&bytes)
            .map_err(|_| invalid_guard("isolation.linux_mounts.mountinfo_utf8"))?;
        self.mount_layout(text, &devices, output.0)
    }

    #[cfg(target_os = "linux")]
    fn mount_layout(&self, text: &str, devices: &[u64], output: u64) -> Result<()> {
        if text.lines().count() > 4096 {
            return Err(invalid_guard("isolation.mount_layout.mount_count_bound"));
        }
        for line in text.lines() {
            let fields = line.split(' ').take(6).collect::<Vec<_>>();
            if fields.len() != 6 {
                return Err(invalid_guard("isolation.mount_layout.mount_fields"));
            }
            let (major, minor) = fields[2].split_once(':').ok_or_else(invalid)?;
            let dev = libc::makedev(
                major.parse().map_err(|_| invalid())?,
                minor.parse().map_err(|_| invalid())?,
            );
            if devices.contains(&dev) && fields[3] != "/" {
                // Conservative refusal of subtree aliases anywhere on a relevant
                // filesystem. The importer has no mount/exclusion authority.
                return Err(invalid_guard("isolation.mount_layout.no_subtree_alias"));
            }
            if dev == output {
                let path = mount_path(fields[4])?;
                // File mounts cannot contain an output directory.
                if std::fs::symlink_metadata(&path)
                    .map_err(|e| invalid_io("isolation.mount_layout.symlink_metadata", e))?
                    .is_file()
                {
                    continue;
                }
                let ancestors = lineage(&Directory::open(&path)?)?;
                if self
                    .roots
                    .iter()
                    .any(|(_, root)| ancestors.iter().skip(1).any(|id| id == root))
                {
                    // A nested mount imports this FS into a protected root.
                    return Err(invalid_guard(
                        "isolation.mount_layout.no_nested_protected_mount",
                    ));
                }
            }
        }
        Ok(())
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
#[cfg(target_os = "linux")]
fn mount_id(directory: &Directory) -> Result<u64> {
    use std::os::fd::AsRawFd;
    let mut stat = std::mem::MaybeUninit::<libc::statx>::uninit();
    let result = unsafe {
        libc::statx(
            directory.0.as_raw_fd(),
            c"".as_ptr(),
            libc::AT_EMPTY_PATH,
            libc::STATX_MNT_ID,
            stat.as_mut_ptr(),
        )
    };
    if result != 0 {
        return Err(invalid_io(
            "isolation.mount_id.statx",
            std::io::Error::last_os_error(),
        ));
    }
    let stat = unsafe { stat.assume_init() };
    if stat.stx_mask & libc::STATX_MNT_ID == 0 {
        return Err(invalid_guard("isolation.mount_id.supported"));
    }
    Ok(stat.stx_mnt_id)
}
#[cfg(target_os = "linux")]
fn mount_path(value: &str) -> Result<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    let mut bytes = Vec::new();
    let mut chars = value.bytes();
    while let Some(byte) = chars.next() {
        if byte == b'\\' {
            let digits = chars.by_ref().take(3).collect::<Vec<_>>();
            let decoded = match digits.as_slice() {
                b"040" => b' ',
                b"011" => b'\t',
                b"012" => b'\n',
                b"134" => b'\\',
                _ => return Err(invalid()),
            };
            bytes.push(decoded);
        } else {
            bytes.push(byte);
        }
    }
    Ok(std::ffi::OsString::from_vec(bytes).into())
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
    #[cfg(target_os = "linux")]
    #[test]
    fn bind_and_nested_mount_views_are_refused_before_output_creation() {
        let root =
            std::env::temp_dir().join(format!("ledger-mount-proof-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let protected = root.join("protected");
        let nested = protected.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        let proof = Proof::new(&[&protected]).unwrap();
        let dev = std::fs::metadata(&root).unwrap().dev();
        let id = format!("{}:{}", libc::major(dev), libc::minor(dev));
        for (mount_root, mountpoint) in [("/subtree", root.as_path()), ("/", nested.as_path())] {
            let record = format!(
                "42 1 {id} {mount_root} {} rw - ext4 none rw\n",
                mountpoint.display()
            );
            assert!(proof.mount_layout(&record, &[dev], dev).is_err());
        }
        assert!(proof
            .mount_layout(&format!("42 1 {id} / / rw - ext4 none rw\n"), &[dev], dev)
            .is_ok());
        assert_eq!(
            mount_path("/with\\040space\\134slash").unwrap(),
            PathBuf::from("/with space\\slash")
        );
        assert!(mount_path("/bad\\999escape").is_err());
        assert!(!root.join("output").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
