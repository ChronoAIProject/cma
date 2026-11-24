//! A refresh job retains its original private-home identity before native effects.

use layer3_adapter::ingress_budget::{IngressOwnershipPolicy, SharedOwned};
use std::fs::File;

#[path = "native_codex_ingress_open.rs"]
pub(super) mod open;

/// Production construction requires the validated fresh-home/namespace contract.
/// Only synthetic descendants can currently mint this otherwise opaque value.
pub(super) struct OwnedPrivateHome {
    directory: File,
    identity: HomeIdentity,
}

struct HomeIdentity {
    device: u64,
    inode: u64,
    generation: u128,
}

pub(super) struct NativeHomeJob {
    generation: u128,
    home: SharedOwned<OwnedPrivateHome>,
}

impl NativeHomeJob {
    fn owns(&self, home: &SharedOwned<OwnedPrivateHome>, policy: &IngressOwnershipPolicy) -> bool {
        SharedOwned::ptr_eq(&self.home, home)
            && self.generation == home.identity.generation
            && self.home.shares_custody_domain(policy)
            && home.shares_custody_domain(policy)
    }
}
