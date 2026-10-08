//! The production gauge and signaller, on Linux x86_64 only.
use super::{MemoryGauge, Signaller};
use std::{
    fs::File,
    io,
    os::{
        fd::{AsRawFd, BorrowedFd},
        unix::fs::FileExt,
    },
    path::Path,
};

/// Reads the container's cgroup v2 memory files. Both are opened once and
/// read with `pread` into stack buffers, so sampling never allocates.
pub struct CgroupV2Gauge {
    limit: u64,
    current: File,
    stat: File,
}

impl CgroupV2Gauge {
    /// Open the cgroup at `root` (`/sys/fs/cgroup` in production). Its
    /// `memory.max` must be a number: without a limit there is nothing to
    /// protect, and the transformer refuses to start.
    pub fn open(root: &Path) -> io::Result<Self> {
        let max = std::fs::read_to_string(root.join("memory.max"))?;
        let limit = max.trim().parse::<u64>().map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "memory.max is not a byte count; the container needs a memory limit",
            )
        })?;
        Ok(Self {
            limit,
            current: File::open(root.join("memory.current"))?,
            stat: File::open(root.join("memory.stat"))?,
        })
    }
}

impl MemoryGauge for CgroupV2Gauge {
    fn limit(&self) -> u64 {
        self.limit
    }

    fn sample(&mut self, _job_pid: u32) -> io::Result<u64> {
        let mut buffer = [0_u8; 32];
        let read = self.current.read_at(&mut buffer, 0)?;
        number(&buffer[..read]).ok_or_else(|| io::ErrorKind::InvalidData.into())
    }

    fn inactive_file(&mut self) -> io::Result<u64> {
        let mut buffer = [0_u8; 8192];
        let read = self.stat.read_at(&mut buffer, 0)?;
        buffer[..read]
            .split(|byte| *byte == b'\n')
            .find_map(|line| line.strip_prefix(b"inactive_file "))
            .and_then(number)
            .ok_or_else(|| io::ErrorKind::InvalidData.into())
    }
}

/// The leading decimal number of `bytes`, without allocating.
fn number(bytes: &[u8]) -> Option<u64> {
    let digits = bytes.iter().take_while(|byte| byte.is_ascii_digit());
    let mut value: u64 = 0;
    let mut seen = false;
    for digit in digits {
        value = value
            .checked_mul(10)?
            .checked_add(u64::from(digit - b'0'))?;
        seen = true;
    }
    seen.then_some(value)
}

/// Sends SIGKILL through a pidfd: never by pid, so never to a reused one.
#[derive(Debug, Default)]
pub struct PidfdSignaller;

impl Signaller for PidfdSignaller {
    fn kill(&mut self, pidfd: BorrowedFd<'_>) -> io::Result<()> {
        // SAFETY: the descriptor is valid while borrowed; a null siginfo
        // makes the kernel fill in a plain kill.
        let result = unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                pidfd.as_raw_fd(),
                libc::SIGKILL,
                std::ptr::null::<libc::siginfo_t>(),
                0_u32,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cgroup(max: &str, current: &str, stat: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("cmaeg-cgroup-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("memory.max"), max).unwrap();
        std::fs::write(root.join("memory.current"), current).unwrap();
        std::fs::write(root.join("memory.stat"), stat).unwrap();
        root
    }

    #[test]
    fn an_unlimited_cgroup_is_refused() {
        let root = cgroup("max\n", "1\n", "");
        assert_eq!(
            CgroupV2Gauge::open(&root).err().map(|error| error.kind()),
            Some(io::ErrorKind::InvalidData)
        );
        std::fs::remove_dir_all(root).unwrap();
        let missing = std::env::temp_dir().join(format!("cmaeg-none-{}", uuid::Uuid::new_v4()));
        assert!(CgroupV2Gauge::open(&missing).is_err());
    }

    #[test]
    fn a_numeric_limit_is_read_and_samples_parse() {
        let stat = "anon 1\nfile 2\nactive_file 3\ninactive_file 4096\nslab 5\n";
        let root = cgroup("2147483648\n", "1073741824\n", stat);
        let mut gauge = CgroupV2Gauge::open(&root).unwrap();
        assert_eq!(gauge.limit(), 2_147_483_648);
        assert_eq!(gauge.sample(0).unwrap(), 1_073_741_824);
        assert_eq!(gauge.inactive_file().unwrap(), 4096);
        // Rewritten in place, as the kernel's files are: re-read, not cached.
        std::fs::write(root.join("memory.current"), "7\n").unwrap();
        assert_eq!(gauge.sample(0).unwrap(), 7);
        std::fs::write(root.join("memory.stat"), "anon 1\n").unwrap();
        assert!(gauge.inactive_file().is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn numbers_parse_without_allocating() {
        assert_eq!(number(b"123\n"), Some(123));
        assert_eq!(number(b"\n"), None);
        assert_eq!(number(b"99999999999999999999"), None);
    }
}
