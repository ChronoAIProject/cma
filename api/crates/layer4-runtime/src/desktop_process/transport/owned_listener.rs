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
        Self::discover_pinned(path, leader, pin_socket(path)?).await
    }

    async fn discover_pinned(
        path: &Path,
        leader: i32,
        file: File,
    ) -> io::Result<(Self, UnixStream)> {
        if !file.metadata()?.file_type().is_socket() {
            return Err(unowned());
        }
        let stream = connect_pinned(&file).await?;
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
        let stream = connect_pinned(&self.file).await?;
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

async fn connect_pinned(file: &File) -> io::Result<UnixStream> {
    // Inspect the peer reached through the same inode retained by this
    // descriptor. The visible pathname is checked only for displacement.
    UnixStream::connect(super::token_map::endpoint(file)).await
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

#[cfg(test)]
mod tests {
    use std::time::Duration;
    use tokio::net::UnixListener;

    use super::*;
    use crate::desktop_process::{transport::token_map, GenerationDirectory};

    #[tokio::test]
    async fn desktop_discovery_reconnect_and_token_map_use_the_pinned_socket() {
        let files = GenerationDirectory::create().unwrap();
        let current = files.path.join("display.sock");
        let displaced = files.path.join("rfb.sock");
        let original = UnixListener::bind(&current).unwrap();
        let leader = unsafe { libc::getsid(0) };

        let pin = pin_socket(&current).unwrap();
        std::fs::rename(&current, &displaced).unwrap();
        let replacement = UnixListener::bind(&current).unwrap();
        assert!(OwnedListener::discover_pinned(&current, leader, pin)
            .await
            .is_err());
        // Discovery reached the pinned socket and then rejected displacement.
        accepted(&original).await;
        no_connection(&replacement).await;
        std::fs::remove_file(&current).unwrap();
        std::fs::rename(&displaced, &current).unwrap();

        let (owned, discovery) = OwnedListener::discover(&current, leader).await.unwrap();
        accepted(&original).await;
        drop(discovery);
        token_map::seal(&files.token_map, "fixture", &owned.pinned_endpoint()).unwrap();
        let map = std::fs::read_to_string(token_map::endpoint(&files.token_map)).unwrap();
        let destination = map
            .trim_end()
            .strip_prefix("fixture: unix_socket:")
            .unwrap();

        std::fs::rename(&current, &displaced).unwrap();
        let replacement = UnixListener::bind(&current).unwrap();
        assert!(owned.connect().await.is_err());
        let pinned = connect_pinned(&owned.file).await.unwrap();
        assert_eq!(peer_in_session(&pinned, leader).unwrap(), owned.peer);
        accepted(&original).await;
        let published = UnixStream::connect(destination).await.unwrap();
        assert_eq!(peer_in_session(&published, leader).unwrap(), owned.peer);
        accepted(&original).await;
        no_connection(&replacement).await;
    }

    async fn accepted(listener: &UnixListener) {
        tokio::time::timeout(Duration::from_secs(1), listener.accept())
            .await
            .unwrap()
            .unwrap();
    }

    async fn no_connection(listener: &UnixListener) {
        assert!(
            tokio::time::timeout(Duration::from_millis(50), listener.accept())
                .await
                .is_err()
        );
    }
}
