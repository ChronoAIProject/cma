//! The job directories under the media root: one random `0700` directory per
//! set, holding one read-only `input`.
//!
//! Jobs open `input` read-only and write only to stdout, so the root holds at
//! most one input at a time; the gateway sweeps it at startup and before
//! every job, keeping only the live job directory.
use bytes::Bytes;
use ring::rand::{SecureRandom, SystemRandom};
use std::{
    fs::{self, DirBuilder, OpenOptions},
    io::{self, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

/// The deployment's memory `emptyDir`; `tests/infra_manifests.rs` pins the
/// mount to this path.
pub const MEDIA_ROOT: &str = "/var/cmaeg/media";
/// The input every job reads from its working directory.
pub const INPUT: &str = "input";

/// The directory job directories are created in.
#[derive(Debug, Clone)]
pub struct MediaRoot {
    path: PathBuf,
}

impl MediaRoot {
    /// `path` is injected so tests use temporary directories.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The root exists, is a directory and accepts a new job directory.
    pub fn check(&self) -> io::Result<()> {
        if !fs::metadata(&self.path)?.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the media root is not a directory",
            ));
        }
        let probe = self.create_job_dir()?;
        let path = probe.path.clone();
        probe.disarm();
        fs::remove_dir_all(path)
    }

    /// Remove every entry except `keep`, returning how many were removed.
    pub fn sweep(&self, keep: Option<&Path>) -> io::Result<usize> {
        let mut removed = 0;
        for entry in fs::read_dir(&self.path)? {
            let entry = entry?;
            let path = entry.path();
            if keep == Some(path.as_path()) {
                continue;
            }
            if entry.file_type()?.is_dir() {
                fs::remove_dir_all(&path)?;
            } else {
                fs::remove_file(&path)?;
            }
            removed += 1;
        }
        Ok(removed)
    }

    /// A new `0700` directory named by 32 random lowercase hex characters.
    pub fn create_job_dir(&self) -> io::Result<JobDir> {
        let mut name = [0_u8; 16];
        SystemRandom::new()
            .fill(&mut name)
            .map_err(|_| io::Error::other("no randomness for a job directory name"))?;
        let name: String = name.iter().map(|byte| format!("{byte:02x}")).collect();
        let path = self.path.join(name);
        DirBuilder::new().mode(0o700).create(&path)?;
        Ok(JobDir { path, armed: true })
    }
}

/// A live job directory, removed by [`JobDir::close`] or, if the owner is
/// cancelled first, synchronously on drop (tmpfs: microseconds).
#[derive(Debug)]
pub struct JobDir {
    path: PathBuf,
    armed: bool,
}

impl JobDir {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Write `input` as a new `0400` file. The file is opened for writing
    /// only while it is created; no job can open it writable.
    pub fn write_input(&self, input: &Bytes) -> io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o400)
            .open(self.path.join(INPUT))?;
        file.write_all(input)
    }

    /// Remove the directory on the blocking pool.
    pub async fn close(mut self) -> io::Result<()> {
        self.armed = false;
        let path = std::mem::take(&mut self.path);
        tokio::task::spawn_blocking(move || fs::remove_dir_all(path))
            .await
            .map_err(|_| io::Error::other("the job directory removal task failed"))?
    }

    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for JobDir {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if let Err(error) = fs::remove_dir_all(&self.path) {
            if error.kind() != io::ErrorKind::NotFound {
                tracing::warn!(
                    kind = ?error.kind(),
                    "media job directory not removed; the next sweep removes it"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn temp_root() -> MediaRoot {
        let path = std::env::temp_dir().join(format!("cmaeg-root-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        MediaRoot::new(path)
    }

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn job_dir_modes_and_sweep() {
        let root = temp_root();
        root.check().unwrap();
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
        let stale_dir = root.create_job_dir().unwrap();
        stale_dir.write_input(&Bytes::from_static(b"old")).unwrap();
        let stale_path = stale_dir.path().to_path_buf();
        stale_dir.disarm();
        fs::write(root.path().join("stray"), b"x").unwrap();

        let live = root.create_job_dir().unwrap();
        let name = live
            .path()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        assert_eq!(name.len(), 32);
        assert!(name
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()));
        assert_eq!(mode(live.path()), 0o700);
        live.write_input(&Bytes::from_static(b"input bytes"))
            .unwrap();
        let input = live.path().join(INPUT);
        assert_eq!(mode(&input), 0o400);
        assert_eq!(fs::read(&input).unwrap(), b"input bytes");
        // A second write cannot replace or reopen the input.
        assert!(live.write_input(&Bytes::from_static(b"again")).is_err());

        assert_eq!(root.sweep(Some(live.path())).unwrap(), 2);
        assert!(!stale_path.exists());
        assert!(input.exists());
        drop(live);
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
        fs::remove_dir(root.path()).unwrap();
    }

    #[tokio::test]
    async fn close_removes_the_directory() {
        let root = temp_root();
        let dir = root.create_job_dir().unwrap();
        dir.write_input(&Bytes::from_static(b"x")).unwrap();
        let path = dir.path().to_path_buf();
        dir.close().await.unwrap();
        assert!(!path.exists());
        assert_eq!(root.sweep(None).unwrap(), 0);
        fs::remove_dir(root.path()).unwrap();
    }

    #[test]
    fn a_missing_or_file_root_fails_its_check() {
        let missing = MediaRoot::new(
            std::env::temp_dir().join(format!("cmaeg-missing-{}", uuid::Uuid::new_v4())),
        );
        assert_eq!(missing.check().unwrap_err().kind(), io::ErrorKind::NotFound);
        let file = std::env::temp_dir().join(format!("cmaeg-file-{}", uuid::Uuid::new_v4()));
        fs::write(&file, b"x").unwrap();
        assert_eq!(
            MediaRoot::new(&file).check().unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        fs::remove_file(file).unwrap();
    }
}
