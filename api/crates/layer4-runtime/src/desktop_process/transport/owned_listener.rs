use std::ffi::CString;
use std::fs::File;
use std::io;
use std::os::fd::FromRawFd as _;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _};
use std::path::{Path, PathBuf};

use tokio::net::UnixStream;

pub(in crate::desktop_process) struct OwnedListener {
    path: PathBuf,
    leader: i32,
    peer: i32,
    // Pin the original socket inode even if the launcher unlinks the pathname.
    file: File,
}

impl OwnedListener {
    pub(super) async fn discover(path: &Path, leader: i32) -> io::Result<(Self, UnixStream)> {
        let file = pin_socket(path)?;
        if !file.metadata()?.file_type().is_socket() {
            return Err(unowned());
        }
        let stream = UnixStream::connect(path).await?;
        let peer = peer_in_session(&stream, leader)?;
        let listener = Self {
            path: path.into(),
            leader,
            peer,
            file,
        };
        listener.check_path()?;
        Ok((listener, stream))
    }

    pub(super) async fn connect(&self) -> io::Result<UnixStream> {
        self.check_path()?;
        let stream = UnixStream::connect(&self.path).await?;
        if peer_in_session(&stream, self.leader)? != self.peer {
            return Err(unowned());
        }
        self.check_path()?;
        Ok(stream)
    }

    pub(super) fn pinned_endpoint(&self) -> String {
        super::token_map::endpoint(&self.file)
    }

    pub(super) fn check_current(&self) -> io::Result<()> {
        self.check_path()?;
        if unsafe { libc::getsid(self.peer) } != self.leader {
            return Err(unowned());
        }
        Ok(())
    }

    fn check_path(&self) -> io::Result<()> {
        let current = self.path.symlink_metadata()?;
        let pinned = self.file.metadata()?;
        if current.file_type().is_socket()
            && (current.dev(), current.ino()) == (pinned.dev(), pinned.ino())
        {
            Ok(())
        } else {
            Err(unowned())
        }
    }
}

fn pin_socket(path: &Path) -> io::Result<File> {
    let path = CString::new(path.as_os_str().as_bytes())?;
    // OpenOptions strips the access-mode bits from custom flags. On musl,
    // O_ACCMODE includes O_PATH, which an open on a socket inode requires.
    let fd = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // The descriptor is newly opened and owned exclusively by this File.
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn peer_in_session(stream: &UnixStream, leader: i32) -> io::Result<i32> {
    let peer = stream
        .peer_cred()?
        .pid()
        .filter(|pid| *pid > 0)
        .ok_or_else(unowned)?;
    // The guardian creates this session before launching any desktop process
    // and retains its managed PID until the generation has been invalidated.
    if unsafe { libc::getsid(peer) } == leader {
        Ok(peer)
    } else {
        Err(unowned())
    }
}

fn unowned() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "desktop listener identity changed",
    )
}
