//! Reversible source moves under an already established exclusive writer hold.
//! Both names stay descriptor-relative; an unexpected entry is never replaced.
use std::ffi::CString;
use std::fs::File;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use super::io::Directory;
use super::{changed, invalid, plan_io::storage};
use crate::failure::Result;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Boundary {
    SourceFileSync,
    BeforeMove,
    AfterMove,
    DestinationFileSync,
    DestinationSync,
    SourceSync,
    Verified,
}

pub(crate) struct Pair {
    active: Directory,
    retained: Directory,
    active_path: PathBuf,
    retained_path: PathBuf,
}

impl Pair {
    pub(crate) fn open(active: &Path, retained: &Path) -> Result<Self> {
        super::isolation::Proof::new(&[active])?.parent(retained)?;
        let result = Self {
            active: Directory::open(active)?,
            retained: Directory::open(retained)?,
            active_path: active.into(),
            retained_path: retained.into(),
        };
        let active = result.active.0.metadata().map_err(|_| invalid())?;
        let retained = result.retained.0.metadata().map_err(|_| invalid())?;
        if active.dev() != retained.dev()
            || active.ino() == retained.ino()
            || active.uid() != unsafe { libc::geteuid() }
            || retained.uid() != unsafe { libc::geteuid() }
            || active.mode() & 0o022 != 0
            || retained.mode() & 0o077 != 0
        {
            return Err(invalid());
        }
        result.verify()?;
        Ok(result)
    }

    fn verify(&self) -> Result<()> {
        for (directory, path, forbidden) in [
            (&self.active, &self.active_path, 0o022),
            (&self.retained, &self.retained_path, 0o077),
        ] {
            let metadata = directory.0.metadata().map_err(|_| invalid())?;
            if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & forbidden != 0 {
                return Err(invalid());
            }
            let expected = directory.identity()?;
            let actual = Directory::open(path)?.identity()?;
            if (actual.device, actual.inode) != (expected.device, expected.inode) {
                return Err(changed());
            }
        }
        Ok(())
    }

    /// The retained archive is verified by the caller before passing its exact
    /// original bytes. Retrying an acknowledged or ambiguous move is identical.
    pub(crate) fn transfer(
        &self,
        name: &str,
        expected: &[u8],
        restore: bool,
        mut boundary: impl FnMut(Boundary) -> Result<()>,
    ) -> Result<()> {
        if !name.strip_suffix(".json").is_some_and(|stem| {
            stem.len() == 64
                && stem
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        }) || expected.is_empty()
            || expected.len() > crate::mutation_journal::MAX_JOURNAL_BYTES as usize
        {
            return Err(invalid());
        }
        self.verify()?;
        let (from, to) = if restore {
            (&self.retained, &self.active)
        } else {
            (&self.active, &self.retained)
        };
        let source = exact(from, name, expected)?;
        let destination = exact(to, name, expected)?;
        match (source, destination) {
            (Some(source), None) => {
                // Persist the retained bytes even if the source was originally
                // created by a writer with weaker durability guarantees.
                boundary(Boundary::SourceFileSync)?;
                source.sync_all().map_err(|_| storage())?;
                boundary(Boundary::BeforeMove)?;
                self.verify()?;
                // Recheck immediately before rename. The external writer hold
                // remains necessary: POSIX has no rename-if-inode operation.
                if identity(&exact(from, name, expected)?.ok_or_else(changed)?)?
                    != identity(&source)?
                {
                    return Err(changed());
                }
                rename(from, to, name)?;
                boundary(Boundary::AfterMove)?;
                if identity(&exact(to, name, expected)?.ok_or_else(changed)?)? != identity(&source)?
                {
                    return Err(changed());
                }
            }
            (None, Some(_)) => {} // Resume after a durable or ambiguous move.
            _ => return Err(changed()),
        }
        // Persist the new name before removal of the old one. On a failed sync
        // no completion is reported, and retry performs both syncs again.
        boundary(Boundary::DestinationFileSync)?;
        exact(to, name, expected)?
            .ok_or_else(changed)?
            .sync_all()
            .map_err(|_| storage())?;
        boundary(Boundary::DestinationSync)?;
        to.0.sync_all().map_err(|_| storage())?;
        boundary(Boundary::SourceSync)?;
        from.0.sync_all().map_err(|_| storage())?;
        self.verify()?;
        if exact(from, name, expected)?.is_some() || exact(to, name, expected)?.is_none() {
            return Err(changed());
        }
        boundary(Boundary::Verified)
    }
}

fn identity(file: &File) -> Result<(u64, u64)> {
    let metadata = file.metadata().map_err(|_| invalid())?;
    Ok((metadata.dev(), metadata.ino()))
}

fn exact(directory: &Directory, name: &str, expected: &[u8]) -> Result<Option<File>> {
    let Some(mut file) = directory.open_at(name, 0)? else {
        return Ok(None);
    };
    let metadata = file.metadata().map_err(|_| invalid())?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o022 != 0
        || metadata.len() != expected.len() as u64
    {
        return Err(invalid());
    }
    let before: super::io::Identity = metadata.into();
    let mut bytes = Vec::with_capacity(expected.len());
    (&mut file)
        .take(expected.len() as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| storage())?;
    let current = directory.open_at(name, 0)?.ok_or_else(changed)?;
    if bytes != expected
        || super::io::Identity::from(file.metadata().map_err(|_| invalid())?) != before
        || super::io::Identity::from(current.metadata().map_err(|_| invalid())?) != before
    {
        return Err(changed());
    }
    Ok(Some(file))
}

fn rename(from: &Directory, to: &Directory, name: &str) -> Result<()> {
    let name = CString::new(name).map_err(|_| invalid())?;
    // SAFETY: pinned directory descriptors and a validated single component.
    // RENAME_NOREPLACE/RENAME_EXCL must never be emulated by check then rename.
    #[cfg(target_os = "linux")]
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            from.0.as_raw_fd(),
            name.as_ptr(),
            to.0.as_raw_fd(),
            name.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    #[cfg(target_os = "macos")]
    let result = unsafe {
        libc::renameatx_np(
            from.0.as_raw_fd(),
            name.as_ptr(),
            to.0.as_raw_fd(),
            name.as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(storage())
    }
}

#[cfg(test)]
#[path = "retirement_move_tests.rs"]
mod tests;
