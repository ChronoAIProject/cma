use super::*;
use std::ffi::{CStr, CString};
use std::os::{
    fd::{AsRawFd, FromRawFd},
    unix::{ffi::OsStrExt, fs::OpenOptionsExt},
};

pub(super) struct Directory(File);
impl Directory {
    pub(super) fn root(path: &Path) -> Result<Self> {
        if !path.is_absolute() {
            return Err(INVALID);
        }
        let mut current = Self(
            std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open("/")
                .map_err(|_| INVALID)?,
        );
        for component in path.components() {
            match component {
                std::path::Component::RootDir => {}
                std::path::Component::Normal(name) => current = current.child(name.as_bytes())?,
                _ => return Err(INVALID),
            }
        }
        Ok(current)
    }
    pub(super) fn attributes(&self, name: Option<&[u8]>) -> Result<Vec<(String, String)>> {
        super::attributes::read(&self.0, name)
    }
    pub(super) fn clone_dir(&self) -> Result<Self> {
        Ok(Self(self.open(b".", libc::O_DIRECTORY)?))
    }
    fn open(&self, name: &[u8], flags: i32) -> Result<File> {
        let name = CString::new(name).map_err(|_| INVALID)?;
        // The parent descriptor is retained and all opens reject symlinks. A
        // replaced path cannot redirect an admitted read outside that directory.
        let fd = unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK | flags,
            )
        };
        if fd < 0 {
            return Err(INVALID);
        }
        Ok(unsafe { File::from_raw_fd(fd) })
    }
    pub(super) fn child(&self, name: &[u8]) -> Result<Self> {
        Ok(Self(self.open(name, libc::O_DIRECTORY)?))
    }
    pub(super) fn file(&self, name: &[u8]) -> Result<File> {
        let file = self.open(name, 0)?;
        if !file.metadata().map_err(|_| INVALID)?.is_file() {
            return Err(INVALID);
        }
        Ok(file)
    }
    pub(super) fn metadata(&self) -> Result<Metadata> {
        self.0.metadata().map_err(|_| INVALID)
    }
    pub(super) fn create_private(&self, name: &[u8]) -> Result<Option<File>> {
        let name = CString::new(name).map_err(|_| INVALID)?;
        let fd = unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                name.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
        };
        if fd >= 0 {
            return Ok(Some(unsafe { File::from_raw_fd(fd) }));
        }
        if std::io::Error::last_os_error().raw_os_error() == Some(libc::EEXIST) {
            Ok(None)
        } else {
            Err(INVALID)
        }
    }
    pub(super) fn sync(&self) -> Result<()> {
        self.0.sync_all().map_err(|_| INVALID)
    }
    pub(super) fn socket(&self, name: &[u8]) -> Result<()> {
        super::socket::restore(&self.0, name)
    }
    pub(super) fn names(&self) -> Result<Vec<Vec<u8>>> {
        // fdopendir owns this independent descriptor, so enumeration never
        // changes the offset or lifetime of the retained source handle.
        use std::os::fd::IntoRawFd;
        let fd = self.open(b".", libc::O_DIRECTORY)?.into_raw_fd();
        let ptr = unsafe { libc::fdopendir(fd) };
        if ptr.is_null() {
            unsafe {
                libc::close(fd);
            }
            return Err(INVALID);
        }
        struct Listing(*mut libc::DIR);
        impl Drop for Listing {
            fn drop(&mut self) {
                unsafe {
                    libc::closedir(self.0);
                }
            }
        }
        let listing = Listing(ptr);
        let mut names = Vec::new();
        loop {
            // errno must distinguish EOF from an enumeration failure.
            unsafe {
                set_errno(0);
            }
            let entry = unsafe { libc::readdir(listing.0) };
            if entry.is_null() {
                if std::io::Error::last_os_error().raw_os_error() != Some(0) {
                    return Err(INVALID);
                }
                break;
            }
            let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
            if name == b"." || name == b".." {
                continue;
            }
            if names.len() >= 200_000 {
                return Err(INVALID);
            }
            names.push(name.to_vec());
        }
        names.sort();
        Ok(names)
    }
    pub(super) fn entry(&self, name: &[u8]) -> Result<(SourceMetadata, Kind)> {
        let name = CString::new(name).map_err(|_| INVALID)?;
        let mut st = std::mem::MaybeUninit::<libc::stat>::uninit();
        if unsafe {
            libc::fstatat(
                self.0.as_raw_fd(),
                name.as_ptr(),
                st.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            return Err(INVALID);
        }
        let st = unsafe { st.assume_init() };
        let kind = match st.st_mode & libc::S_IFMT {
            libc::S_IFDIR => Kind::Directory,
            libc::S_IFREG => Kind::File,
            libc::S_IFSOCK => Kind::Socket,
            libc::S_IFIFO => Kind::Fifo,
            libc::S_IFLNK => {
                let mut bytes = vec![0u8; 65536];
                let size = unsafe {
                    libc::readlinkat(
                        self.0.as_raw_fd(),
                        name.as_ptr(),
                        bytes.as_mut_ptr().cast(),
                        bytes.len(),
                    )
                };
                if size < 0 || size as usize >= bytes.len() {
                    return Err(INVALID);
                }
                bytes.truncate(size as usize);
                Kind::Symlink {
                    target: STANDARD.encode(bytes),
                }
            }
            _ => return Err(INVALID),
        };
        // POSIX stat values are copied explicitly; no struct transmute or
        // path-based metadata lookup introduces a second filesystem authority.
        let metadata = stat_metadata(&st);
        Ok((metadata, kind))
    }
}
#[cfg(target_os = "macos")]
unsafe fn set_errno(value: i32) {
    unsafe {
        *libc::__error() = value;
    }
}
#[cfg(target_os = "linux")]
unsafe fn set_errno(value: i32) {
    unsafe {
        *libc::__errno_location() = value;
    }
}

#[allow(clippy::unnecessary_cast)] // POSIX integer widths differ on macOS/Linux.
fn stat_metadata(s: &libc::stat) -> SourceMetadata {
    SourceMetadata {
        device: s.st_dev as u64,
        inode: s.st_ino as u64,
        mode: s.st_mode as u32,
        uid: s.st_uid,
        gid: s.st_gid,
        length: s.st_size as u64,
        links: s.st_nlink as u64,
        modified_seconds: s.st_mtime as i64,
        modified_nanos: s.st_mtime_nsec as i64,
        changed_seconds: s.st_ctime as i64,
        changed_nanos: s.st_ctime_nsec as i64,
    }
}
