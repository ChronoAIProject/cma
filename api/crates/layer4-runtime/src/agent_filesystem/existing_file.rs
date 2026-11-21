//! Replacing file contents can preserve mode only for the same access principals.
use std::{
    fs::File,
    io::{self, Seek as _, SeekFrom},
    mem::MaybeUninit,
    os::fd::AsRawFd as _,
};

use super::io::{open_at, read_held_regular, rename, stat};
use super::{conflict, digest, io_error, rejected, Result, MAX_AGENT_FILE_BYTES};

pub(super) struct Original {
    descriptor: File,
    metadata: libc::stat,
    directory: libc::stat,
    version: String,
}

impl Original {
    pub(super) fn open(parent: &File, name: &str, observed: &libc::stat) -> Result<Self> {
        let directory = descriptor_stat(parent)?;
        let descriptor = open_at(parent, name, libc::O_RDONLY, 0)?;
        let metadata = descriptor_stat(&descriptor)?;
        if !same_file(observed, &metadata) {
            return Err(conflict());
        }
        let version = read_digest(&descriptor)?;
        if !same_file(&metadata, &descriptor_stat(&descriptor)?)
            || !same_file(&metadata, &stat(parent, name)?)
        {
            return Err(conflict());
        }
        Ok(Self {
            descriptor,
            metadata,
            directory,
            version,
        })
    }

    pub(super) fn digest(&self) -> &str {
        &self.version
    }

    pub(super) fn restore_mode(
        &self,
        parent: &File,
        name: &str,
        stage_name: &str,
        stage: &File,
        set_mode: impl FnOnce(&File, libc::mode_t) -> Result<()>,
    ) -> Result<()> {
        self.revalidate(parent, name)?;
        let metadata = self.revalidate_stage(parent, stage_name, stage)?;
        if metadata.st_mode & 0o7777 & !0o600 != 0 {
            return Err(policy_refused());
        }
        set_mode(stage, self.metadata.st_mode & 0o777)
    }

    pub(super) fn publish(
        &self,
        parent: &File,
        name: &str,
        stage_name: &str,
        stage: &File,
    ) -> Result<()> {
        self.revalidate(parent, name)?;
        let metadata = self.revalidate_stage(parent, stage_name, stage)?;
        if metadata.st_mode & 0o7777 != self.metadata.st_mode & 0o777 {
            return Err(policy_refused());
        }
        rename(parent, stage_name, name)
    }

    fn revalidate(&self, parent: &File, name: &str) -> Result<()> {
        if !same_security(&self.directory, &descriptor_stat(parent)?)
            || !same_file(&self.metadata, &descriptor_stat(&self.descriptor)?)
            || read_digest(&self.descriptor)? != self.version
        {
            return Err(conflict());
        }
        require_no_acl(&self.descriptor)?;
        if !same_file(&self.metadata, &stat(parent, name).map_err(|_| conflict())?) {
            return Err(conflict());
        }
        Ok(())
    }

    fn revalidate_stage(&self, parent: &File, name: &str, stage: &File) -> Result<libc::stat> {
        let metadata = descriptor_stat(stage)?;
        if metadata.st_mode & libc::S_IFMT != libc::S_IFREG
            || metadata.st_nlink != 1
            || metadata.st_uid != self.metadata.st_uid
            || metadata.st_gid != self.metadata.st_gid
        {
            return Err(policy_refused());
        }
        require_no_acl(stage)?;
        if !same_file(&metadata, &stat(parent, name).map_err(|_| conflict())?) {
            return Err(conflict());
        }
        Ok(metadata)
    }
}

fn same_security(before: &libc::stat, after: &libc::stat) -> bool {
    before.st_dev == after.st_dev
        && before.st_ino == after.st_ino
        && before.st_uid == after.st_uid
        && before.st_gid == after.st_gid
        && before.st_mode == after.st_mode
}

fn same_file(before: &libc::stat, after: &libc::stat) -> bool {
    same_security(before, after)
        && before.st_nlink == after.st_nlink
        && before.st_size == after.st_size
        && before.st_mtime == after.st_mtime
        && before.st_mtime_nsec == after.st_mtime_nsec
        && before.st_ctime == after.st_ctime
        && before.st_ctime_nsec == after.st_ctime_nsec
}

pub(super) fn check_parent_binding(root: &File, path: &str, retained: &File) -> Result<()> {
    let (current, _) = super::io::parent(root, path, false).map_err(|_| conflict())?;
    if !same_security(&descriptor_stat(retained)?, &descriptor_stat(&current)?) {
        return Err(conflict());
    }
    Ok(())
}

fn read_digest(mut descriptor: &File) -> Result<String> {
    descriptor.seek(SeekFrom::Start(0)).map_err(io_error)?;
    Ok(digest(&read_held_regular(
        descriptor,
        MAX_AGENT_FILE_BYTES,
    )?))
}

fn descriptor_stat(descriptor: &File) -> Result<libc::stat> {
    let mut metadata = MaybeUninit::<libc::stat>::uninit();
    // SAFETY: the descriptor and writable stat buffer remain live for the call.
    if unsafe { libc::fstat(descriptor.as_raw_fd(), metadata.as_mut_ptr()) } != 0 {
        return Err(io_error(io::Error::last_os_error()));
    }
    // SAFETY: a successful fstat initialized the buffer.
    Ok(unsafe { metadata.assume_init() })
}

pub(super) fn set_mode(stage: &File, mode: libc::mode_t) -> Result<()> {
    // SAFETY: stage is the live owned staging descriptor. The caller passes
    // the authorized original's ordinary access bits.
    if unsafe { libc::fchmod(stage.as_raw_fd(), mode) } != 0 {
        return Err(io_error(io::Error::last_os_error()));
    }
    Ok(())
}

pub(super) fn cleanup_stage(parent: &File, name: &str, stage: &File) -> Result<()> {
    let metadata = descriptor_stat(stage)?;
    let current = stat(parent, name)?;
    if metadata.st_dev == current.st_dev && metadata.st_ino == current.st_ino {
        super::io::unlink(parent, name, false)?;
    }
    Ok(())
}

fn policy_refused() -> crate::failure::RuntimeError {
    rejected("cannot preserve existing file access policy")
}

#[cfg(target_os = "linux")]
fn require_no_acl(descriptor: &File) -> Result<()> {
    // SAFETY: a zero-length query requires no output buffer. The held file
    // descriptor identifies the exact inode whose Linux access ACL is queried.
    let size = unsafe {
        libc::fgetxattr(
            descriptor.as_raw_fd(),
            c"system.posix_acl_access".as_ptr(),
            std::ptr::null_mut(),
            0,
        )
    };
    let errno = (size < 0)
        .then(|| io::Error::last_os_error().raw_os_error())
        .flatten();
    require_absence(size, errno)
}

#[cfg(target_os = "linux")]
fn require_absence(size: isize, errno: Option<i32>) -> Result<()> {
    if size < 0 && errno == Some(libc::ENODATA) {
        Ok(())
    } else {
        Err(policy_refused())
    }
}

#[cfg(not(target_os = "linux"))]
fn require_no_acl(_descriptor: &File) -> Result<()> {
    Err(policy_refused())
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn requires_an_explicit_no_acl_result() {
        assert!(require_absence(-1, Some(libc::ENODATA)).is_ok());
        for (size, errno) in [
            (0, None),
            (32, None),
            (-1, Some(libc::ENOTSUP)),
            (-1, Some(libc::EACCES)),
            (-1, None),
        ] {
            assert!(require_absence(size, errno).is_err());
        }
    }
}
