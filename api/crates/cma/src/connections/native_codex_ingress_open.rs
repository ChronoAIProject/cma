//! Directory-relative original-cache opening for a qualified native supervisor.

use std::{
    fs::File,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::MetadataExt as _,
    },
};

use layer3_adapter::ingress_budget::{
    sizing, AdmissionRefusal, AllocationPlan, IngressClass, IngressOwnershipPolicy,
    IngressWorkScope, SharedOwned,
};

use super::super::{ingress_cache::CacheRead, SecretValue, SubscriptionError};
use super::{NativeHomeJob, OwnedPrivateHome};
use crate::connections::subscription_tokens::CacheDecodeError;

pub(in crate::connections::native_codex) struct PreparedCacheOpen {
    read: CacheRead,
    home: SharedOwned<OwnedPrivateHome>,
    storage: IngressWorkScope,
}

/// The descriptor has one cursor owner. No clone, raw-fd accessor, File accessor
/// or Deref implementation is exposed. The supervisor retains this on errors.
pub(in crate::connections::native_codex) struct OpenedHomeCache {
    file: File,
    read: Option<CacheRead>,
    home: SharedOwned<OwnedPrivateHome>,
    storage: IngressWorkScope,
}

#[cfg_attr(
    not(test),
    expect(dead_code, reason = "native home and executor qualification pending")
)]
impl PreparedCacheOpen {
    pub(in crate::connections::native_codex) fn admit(
        job: &NativeHomeJob,
        home: &SharedOwned<OwnedPrivateHome>,
        input: &SharedOwned<SecretValue>,
        policy: &IngressOwnershipPolicy,
    ) -> Result<Self, CacheDecodeError> {
        if !job.owns(home, policy) || !input.shares_custody_domain(policy) {
            return Err(AdmissionRefusal::InvalidPlan {
                phase: "native cache open ownership",
            }
            .into());
        }
        // The scope's custody allocation is bounded by the existing shared-owner
        // layout. File/kernel state and outer task placement are separate terms.
        let storage = policy.admit(
            IngressClass::Data,
            AllocationPlan::checked(
                "native cache open custody",
                &[sizing::shared_owner_bytes::<()>()],
            )?,
        )?;
        // Complete byte admission before the first open. On refusal the scope
        // has not been allocated and no filesystem operation has run.
        let read = CacheRead::admit(input, policy)?;
        Ok(Self {
            read,
            home: home.clone(),
            storage: IngressWorkScope::new(storage),
        })
    }

    pub(in crate::connections::native_codex) fn open(
        self,
    ) -> Result<OpenedHomeCache, CacheDecodeError> {
        let metadata = self
            .home
            .directory
            .metadata()
            .map_err(|_| CacheDecodeError::Account(SubscriptionError::InvalidAccount))?;
        if !metadata.is_dir()
            || metadata.dev() != self.home.identity.device
            || metadata.ino() != self.home.identity.inode
        {
            return Err(AdmissionRefusal::InvalidPlan {
                phase: "native original home descriptor",
            }
            .into());
        }
        // One fixed component avoids traversal and CString allocation. NONBLOCK
        // allows fstat to reject a FIFO without waiting for a writer; NOCTTY
        // prevents a nonregular object from becoming a controlling terminal.
        let flags =
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK | libc::O_NOCTTY;
        // SAFETY: the retained directory owns a live fd; the C literal is
        // NUL-terminated and contains no slash. No create flag needs a mode.
        let fd = unsafe {
            libc::openat(
                self.home.directory.as_raw_fd(),
                c"auth.json".as_ptr(),
                flags,
            )
        };
        if fd < 0 {
            return Err(open_error(std::io::Error::last_os_error()));
        }
        // SAFETY: a successful openat returned one newly owned descriptor. It
        // enters File immediately, before any subsequent fallible operation.
        let file = unsafe { File::from_raw_fd(fd) };
        let metadata = file.metadata();
        self.finish_open(file, metadata)
    }

    fn finish_open(
        self,
        file: File,
        metadata: std::io::Result<std::fs::Metadata>,
    ) -> Result<OpenedHomeCache, CacheDecodeError> {
        let metadata =
            metadata.map_err(|_| CacheDecodeError::Account(SubscriptionError::InvalidAccount))?;
        if !metadata.is_file() || metadata.nlink() != 1 {
            return Err(CacheDecodeError::Account(SubscriptionError::InvalidAccount));
        }
        Ok(OpenedHomeCache {
            file,
            read: Some(self.read),
            home: self.home,
            storage: self.storage,
        })
    }
}

#[cfg_attr(
    not(test),
    expect(dead_code, reason = "native home and executor qualification pending")
)]
impl OpenedHomeCache {
    pub(in crate::connections::native_codex) fn read(mut self) -> CacheReadCompletion {
        let outcome = self
            .read
            .take()
            .ok_or(CacheDecodeError::Refused(AdmissionRefusal::InvalidPlan {
                phase: "native cache read already consumed",
            }))
            .and_then(|read| read.read_file(&mut self.file));
        CacheReadCompletion {
            file: self,
            outcome,
        }
    }

    pub(in crate::connections::native_codex) fn shares_custody_domain(
        &self,
        policy: &IngressOwnershipPolicy,
    ) -> bool {
        self.home.shares_custody_domain(policy) && self.storage.shares_custody_domain(policy)
    }
}

#[cfg_attr(
    not(test),
    expect(dead_code, reason = "native supervisor qualification pending")
)]
pub(in crate::connections::native_codex) struct CacheReadCompletion {
    pub(in crate::connections::native_codex) file: OpenedHomeCache,
    pub(in crate::connections::native_codex) outcome:
        Result<SharedOwned<SecretValue>, CacheDecodeError>,
}

fn open_error(error: std::io::Error) -> CacheDecodeError {
    let classification = match error.raw_os_error() {
        Some(libc::ENOENT | libc::ENOTDIR | libc::ELOOP) => SubscriptionError::InvalidAccount,
        _ => SubscriptionError::Unavailable,
    };
    CacheDecodeError::Account(classification)
}

#[cfg(test)]
#[path = "native_codex_ingress_open_tests.rs"]
mod tests;
