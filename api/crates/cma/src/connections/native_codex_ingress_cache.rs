//! Bounded plaintext storage for the future admitted native file worker.

use std::{
    fs::File,
    io::{Read, Seek as _},
};

use layer3_adapter::ingress_budget::{
    sizing, AdmissionRefusal, AllocationPlan, IngressClass, IngressOwnershipPolicy,
    ReservationBundle, SharedOwned,
};
use zeroize::Zeroizing;

use super::{SecretValue, SubscriptionError, SubscriptionTokens};
use crate::connections::subscription_tokens::CacheDecodeError;

const MAX_CACHE_BYTES: usize = 128 * 1024;
const READ_BYTES: usize = MAX_CACHE_BYTES + 1;

/// This owns only the byte buffer and returned cache. The caller must supply
/// an opened handle tied to its private home, plus blocking-work/cleanup custody.
/// In particular, moving this into ordinary spawn_blocking is not sufficient.
pub(super) struct CacheRead {
    bytes: Zeroizing<Vec<u8>>,
    storage: ReservationBundle,
    policy: IngressOwnershipPolicy,
}

#[cfg_attr(
    all(
        not(test),
        not(all(
            any(
                all(target_os = "linux", target_env = "gnu"),
                all(
                    target_os = "linux",
                    target_env = "musl",
                    target_vendor = "unknown",
                    target_pointer_width = "64",
                    target_endian = "little",
                ),
                target_os = "macos",
            ),
            any(target_arch = "x86_64", target_arch = "aarch64"),
        )),
    ),
    expect(dead_code, reason = "native cache opener unavailable on this target")
)]
impl CacheRead {
    pub(super) fn admit(
        input: &SharedOwned<SecretValue>,
        policy: &IngressOwnershipPolicy,
    ) -> Result<Self, CacheDecodeError> {
        if !input.shares_custody_domain(policy) {
            return Err(AdmissionRefusal::InvalidPlan {
                phase: "native cache read ownership",
            }
            .into());
        }
        let storage = policy.admit(IngressClass::Data, read_plan()?)?;
        Ok(Self {
            bytes: Zeroizing::new(vec![0; READ_BYTES]),
            storage,
            policy: policy.clone(),
        })
    }

    /// Validate the opened object, then read that same descriptor. No path is
    /// reopened and no metadata length is used as an allocation bound.
    pub(super) fn read_file(
        self,
        file: &mut File,
    ) -> Result<SharedOwned<SecretValue>, CacheDecodeError> {
        let metadata = file
            .metadata()
            .map_err(|_| CacheDecodeError::Account(SubscriptionError::InvalidAccount))?;
        if !metadata.is_file() {
            return Err(CacheDecodeError::Account(SubscriptionError::InvalidAccount));
        }
        file.rewind()
            .map_err(|_| CacheDecodeError::Account(SubscriptionError::Unavailable))?;
        self.read(file)
    }

    fn read(
        mut self,
        reader: &mut impl Read,
    ) -> Result<SharedOwned<SecretValue>, CacheDecodeError> {
        let mut used = 0;
        while used < READ_BYTES {
            match reader.read(&mut self.bytes[used..]) {
                Ok(0) => break,
                Ok(count) => used += count,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return Err(CacheDecodeError::Account(SubscriptionError::Unavailable)),
            }
        }
        if used > MAX_CACHE_BYTES {
            return Err(CacheDecodeError::Account(SubscriptionError::InvalidAccount));
        }
        // Truncation preserves capacity. UTF-8 validation moves the same backing
        // into String; its unused, initially zeroed suffix contains no input.
        self.bytes.truncate(used);
        let bytes = std::mem::take(&mut *self.bytes);
        let text = match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(error) => {
                drop(Zeroizing::new(error.into_bytes()));
                return Err(CacheDecodeError::Account(SubscriptionError::Unavailable));
            }
        };
        let cache = self.storage.initialize(|| SecretValue::new(text));
        // The original bytes are owned before the existing admitted decoder
        // allocates. Its transient token copies die before the cache escapes.
        drop(SubscriptionTokens::from_native_cache_admitted(
            &cache,
            &self.policy,
        )?);
        Ok(cache)
    }
}

fn read_plan() -> Result<AllocationPlan, AdmissionRefusal> {
    AllocationPlan::checked(
        "native original cache read",
        &[READ_BYTES, sizing::shared_owner_bytes::<SecretValue>()],
    )
}

#[cfg(test)]
#[path = "native_codex_ingress_cache_tests.rs"]
mod tests;
