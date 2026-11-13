//! Mount topology validation for descriptor-relative output isolation.
use super::super::{invalid, invalid_guard, invalid_io};
#[cfg(target_os = "linux")]
use super::identity;
use super::{lineage, Directory, Proof};
use crate::failure::Result;
use std::path::PathBuf;

impl Proof {
    #[cfg(target_os = "linux")]
    pub(super) fn linux_mounts(&self, parent: &Directory) -> Result<()> {
        let output = identity(parent)?;
        let devices = std::iter::once(output.0)
            .chain(self.roots.iter().map(|(_, id)| id.0))
            .map(device)
            .collect::<Vec<_>>();
        consistent(
            || self.snapshot(parent),
            |snapshot| {
                for ((_, id), mount) in self.roots.iter().zip(&snapshot.root_mounts) {
                    if (id.0 == output.0 && *mount != snapshot.output_mount)
                        || snapshot.ancestors.contains(id)
                    {
                        return Err(invalid_guard("isolation.linux_mounts.same_filesystem_view"));
                    }
                }
                self.mount_layout(&snapshot.text, &devices, device(output.0))
            },
        )
    }

    #[cfg(target_os = "linux")]
    fn snapshot(&self, parent: &Directory) -> Result<Snapshot> {
        let output_mount = mount_id(parent)?;
        let mut root_mounts = Vec::new();
        for (path, expected) in &self.roots {
            let directory = Directory::open(path)?;
            if identity(&directory)? != *expected {
                return Err(super::super::changed());
            }
            root_mounts.push(mount_id(&directory)?);
        }
        Ok(Snapshot {
            text: mountinfo()?,
            output_mount,
            root_mounts,
            ancestors: lineage(parent)?,
        })
    }

    fn mount_layout(&self, text: &str, devices: &[(u32, u32)], output: (u32, u32)) -> Result<()> {
        if text.lines().count() > 4096 {
            return Err(invalid_guard("isolation.mount_layout.mount_count_bound"));
        }
        for line in text.lines() {
            let fields = line.split(' ').take(6).collect::<Vec<_>>();
            if fields.len() != 6 {
                return Err(invalid_guard("isolation.mount_layout.mount_fields"));
            }
            let (major, minor) = fields[2].split_once(':').ok_or_else(invalid)?;
            let dev = (
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

// Keep this independent of mountinfo I/O so the read/inspection race can be
// tested deterministically without requiring privileged mount operations.
fn consistent<T: PartialEq>(
    mut snapshot: impl FnMut() -> Result<T>,
    mut inspect: impl FnMut(&T) -> Result<()>,
) -> Result<()> {
    for attempt in 0..3 {
        let before = snapshot()?;
        let inspected = inspect(&before);
        // A vanished mountpoint is not evidence of bad input when the mount
        // table changed underneath it. Equally, a successful old inspection
        // cannot certify a newly added alias. Recheck both outcomes.
        if before == snapshot()? {
            return inspected;
        }
        tracing::debug!(
            attempt,
            "ledger import mount topology changed during isolation check"
        );
    }
    Err(super::super::changed())
}

#[cfg(target_os = "linux")]
#[derive(PartialEq)]
struct Snapshot {
    text: String,
    output_mount: u64,
    root_mounts: Vec<u64>,
    ancestors: Vec<(u64, u64)>,
}

#[cfg(target_os = "linux")]
fn mountinfo() -> Result<String> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open("/proc/self/mountinfo")
        .map_err(|e| invalid_io("isolation.linux_mounts.open_mountinfo", e))?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| invalid_io("isolation.linux_mounts.read_mountinfo", e))?;
    if bytes.len() > 1024 * 1024 {
        return Err(invalid_guard("isolation.linux_mounts.mountinfo_byte_bound"));
    }
    String::from_utf8(bytes).map_err(|_| invalid_guard("isolation.linux_mounts.mountinfo_utf8"))
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

#[cfg(target_os = "linux")]
fn device(dev: u64) -> (u32, u32) {
    (libc::major(dev), libc::minor(dev))
}

#[cfg(test)]
#[path = "isolation_mounts_tests.rs"]
mod tests;
