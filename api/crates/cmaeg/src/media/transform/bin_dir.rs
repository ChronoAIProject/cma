//! The directory the sandbox executes `ffmpeg` and `ffprobe` from.
//!
//! The job wrapper executes whatever binary sits there, so the directory and
//! both files must be owned by the expected owner (root in production) and
//! writable by nobody else: otherwise the gateway's own uid, which a
//! compromised job also runs as, could replace the decoder the next job
//! executes. Symlinks are refused for the same reason, and a canonical path
//! leaves no `..` or link component to redirect.
use std::{
    fmt,
    fs::{self, Metadata},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

/// The two programs a job may execute.
pub const PROGRAMS: [&str; 2] = ["ffmpeg", "ffprobe"];
/// Group- and other-writable bits.
const SHARED_WRITE: u32 = 0o022;
/// Any execute bit.
const EXECUTABLE: u32 = 0o111;

/// A validated binary directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfmpegDir {
    path: PathBuf,
}

/// Why a binary directory was refused. Paths are operator configuration,
/// not secrets, so they may be named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BinDirError {
    NotAbsolute,
    NotCanonical,
    /// The path or one of the programs could not be inspected.
    Unreadable {
        name: String,
    },
    NotADirectory,
    NotARegularFile {
        name: &'static str,
    },
    WrongOwner {
        name: String,
        uid: u32,
    },
    SharedWritable {
        name: String,
    },
    NotExecutable {
        name: &'static str,
    },
}

impl fmt::Display for BinDirError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAbsolute => formatter.write_str("the directory must be an absolute path"),
            Self::NotCanonical => {
                formatter.write_str("the directory must be canonical (no symlink, `.` or `..`)")
            }
            Self::Unreadable { name } => write!(formatter, "{name} cannot be inspected"),
            Self::NotADirectory => formatter.write_str("the path is not a directory"),
            Self::NotARegularFile { name } => {
                write!(formatter, "{name} must be a regular file, not a symlink")
            }
            Self::WrongOwner { name, uid } => {
                write!(
                    formatter,
                    "{name} is owned by uid {uid}, not the expected owner"
                )
            }
            Self::SharedWritable { name } => {
                write!(formatter, "{name} must not be group- or other-writable")
            }
            Self::NotExecutable { name } => write!(formatter, "{name} is not executable"),
        }
    }
}

impl std::error::Error for BinDirError {}

impl FfmpegDir {
    /// Validate `path` and both programs in it against `expected_owner`
    /// (0 in production; tests pass their own uid).
    pub fn validate(path: &Path, expected_owner: u32) -> Result<Self, BinDirError> {
        if !path.is_absolute() {
            return Err(BinDirError::NotAbsolute);
        }
        let unreadable = |name: &str| BinDirError::Unreadable { name: name.into() };
        let canonical = fs::canonicalize(path).map_err(|_| unreadable("the directory"))?;
        if canonical != path {
            return Err(BinDirError::NotCanonical);
        }
        let directory = fs::metadata(path).map_err(|_| unreadable("the directory"))?;
        if !directory.is_dir() {
            return Err(BinDirError::NotADirectory);
        }
        owned_and_private(&directory, "the directory", expected_owner)?;
        for name in PROGRAMS {
            let program = fs::symlink_metadata(path.join(name)).map_err(|_| unreadable(name))?;
            if !program.file_type().is_file() {
                return Err(BinDirError::NotARegularFile { name });
            }
            owned_and_private(&program, name, expected_owner)?;
            if program.mode() & EXECUTABLE == 0 {
                return Err(BinDirError::NotExecutable { name });
            }
        }
        Ok(Self {
            path: path.to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn owned_and_private(metadata: &Metadata, name: &str, owner: u32) -> Result<(), BinDirError> {
    if metadata.uid() != owner {
        return Err(BinDirError::WrongOwner {
            name: name.into(),
            uid: metadata.uid(),
        });
    }
    if metadata.mode() & SHARED_WRITE != 0 {
        return Err(BinDirError::SharedWritable { name: name.into() });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};

    /// A canonical 0755 directory holding 0755 `ffmpeg` and `ffprobe`, and
    /// the uid that owns it (the test's own).
    struct Fixture {
        dir: PathBuf,
        uid: u32,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("cmaeg-bin-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&dir).unwrap();
            let dir = fs::canonicalize(dir).unwrap();
            chmod(&dir, 0o755);
            for name in PROGRAMS {
                fs::write(dir.join(name), b"#!/bin/true\n").unwrap();
                chmod(&dir.join(name), 0o755);
            }
            let uid = fs::metadata(&dir).unwrap().uid();
            Self { dir, uid }
        }

        fn validate(&self) -> Result<FfmpegDir, BinDirError> {
            FfmpegDir::validate(&self.dir, self.uid)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    fn chmod(path: &Path, mode: u32) {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn a_private_directory_of_regular_executables_is_accepted() {
        let fixture = Fixture::new();
        assert_eq!(fixture.validate().unwrap().path(), fixture.dir);
    }

    #[test]
    fn relative_and_non_canonical_paths_are_refused() {
        let fixture = Fixture::new();
        assert_eq!(
            FfmpegDir::validate(Path::new("opt/ffmpeg/bin"), fixture.uid),
            Err(BinDirError::NotAbsolute)
        );
        let name = fixture.dir.file_name().unwrap();
        let dotted = fixture.dir.join("..").join(name);
        assert_eq!(
            FfmpegDir::validate(&dotted, fixture.uid),
            Err(BinDirError::NotCanonical)
        );
        let missing = fixture.dir.join("missing");
        assert!(matches!(
            FfmpegDir::validate(&missing, fixture.uid),
            Err(BinDirError::Unreadable { .. })
        ));
        assert_eq!(
            FfmpegDir::validate(&fixture.dir.join("ffmpeg"), fixture.uid),
            Err(BinDirError::NotADirectory)
        );
    }

    #[test]
    fn group_or_other_writable_entries_are_refused() {
        let fixture = Fixture::new();
        chmod(&fixture.dir, 0o775);
        assert_eq!(
            fixture.validate(),
            Err(BinDirError::SharedWritable {
                name: "the directory".into()
            })
        );
        chmod(&fixture.dir, 0o755);
        chmod(&fixture.dir.join("ffmpeg"), 0o757);
        assert_eq!(
            fixture.validate(),
            Err(BinDirError::SharedWritable {
                name: "ffmpeg".into()
            })
        );
    }

    #[test]
    fn a_missing_symlinked_or_non_executable_program_is_refused() {
        let fixture = Fixture::new();
        fs::remove_file(fixture.dir.join("ffprobe")).unwrap();
        assert_eq!(
            fixture.validate(),
            Err(BinDirError::Unreadable {
                name: "ffprobe".into()
            })
        );
        fs::write(fixture.dir.join("ffprobe"), b"").unwrap();
        chmod(&fixture.dir.join("ffprobe"), 0o755);
        fs::rename(fixture.dir.join("ffmpeg"), fixture.dir.join("real-ffmpeg")).unwrap();
        symlink(fixture.dir.join("real-ffmpeg"), fixture.dir.join("ffmpeg")).unwrap();
        assert_eq!(
            fixture.validate(),
            Err(BinDirError::NotARegularFile { name: "ffmpeg" })
        );
        fs::remove_file(fixture.dir.join("ffmpeg")).unwrap();
        fs::rename(fixture.dir.join("real-ffmpeg"), fixture.dir.join("ffmpeg")).unwrap();
        assert!(fixture.validate().is_ok());
        chmod(&fixture.dir.join("ffprobe"), 0o644);
        assert_eq!(
            fixture.validate(),
            Err(BinDirError::NotExecutable { name: "ffprobe" })
        );
    }

    #[test]
    fn an_owner_other_than_the_expected_one_is_refused() {
        let fixture = Fixture::new();
        // Production expects root; a test that runs as root cannot show it.
        if fixture.uid == 0 {
            eprintln!("skipped: running as root, which owns the fixture");
            return;
        }
        assert_eq!(
            FfmpegDir::validate(&fixture.dir, 0),
            Err(BinDirError::WrongOwner {
                name: "the directory".into(),
                uid: fixture.uid
            })
        );
        let error = FfmpegDir::validate(&fixture.dir, 0).unwrap_err();
        assert!(error.to_string().contains("not the expected owner"));
    }
}
