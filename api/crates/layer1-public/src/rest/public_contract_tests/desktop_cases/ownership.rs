//! In-memory contract ports; real Mongo and receiver fencing live in CMA/runtime tests.
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Mutex,
};

use layer2_core::{
    sandbox::{
        store::{DesktopSessionRequest, SandboxSessionStore, SandboxStoreError},
        DesktopAuthorization, DesktopError,
    },
    Caller, SandboxId,
};
use layer3_adapter::{
    AdapterError, DesktopOwnership, DesktopOwnershipChallenge, DesktopOwnershipControl,
};

#[derive(Default)]
pub(super) struct Ownership {
    subject: &'static str,
    pub state: Mutex<State>,
    pub checks: AtomicUsize,
    pub revoked: AtomicBool,
}

#[derive(Default)]
pub(super) struct State {
    pub claim: Option<DesktopOwnership>,
    pub receiver: Option<DesktopOwnership>,
    epoch: u64,
    nonce: u64,
    challenge: Option<String>,
    pub writes: usize,
    pub opens: usize,
    pub frames: usize,
    pub releases: usize,
}

pub(super) fn same(left: &DesktopOwnership, right: &DesktopOwnership) -> bool {
    left.sandbox_id == right.sandbox_id
        && left.runtime_id == right.runtime_id
        && left.boot_id == right.boot_id
        && left.session_id == right.session_id
        && left.epoch == right.epoch
}

impl Ownership {
    pub fn new(subject: &'static str) -> Self {
        Self {
            subject,
            ..Default::default()
        }
    }

    pub fn challenge(&self) -> DesktopOwnershipChallenge {
        let mut state = self.state.lock().unwrap();
        state.nonce += 1;
        let nonce = format!("contract-challenge-{}", state.nonce);
        state.challenge = Some(nonce.clone());
        DesktopOwnershipChallenge {
            nonce,
            valid_for_ms: 10_000,
        }
    }

    pub fn confirm(&self, owner: &DesktopOwnership, challenge: &DesktopOwnershipChallenge) {
        owner.validate().unwrap();
        let mut state = self.state.lock().unwrap();
        assert!(state.claim.as_ref() == Some(owner));
        assert_eq!(
            state.challenge.take().as_deref(),
            Some(challenge.nonce.as_str())
        );
        assert!(state
            .receiver
            .as_ref()
            .is_none_or(|current| same(current, owner)));
        state.receiver = Some(owner.clone());
    }

    pub fn frame(&self, owner: &DesktopOwnership) -> layer3_adapter::Result<()> {
        let mut state = self.state.lock().unwrap();
        if !state
            .receiver
            .as_ref()
            .is_some_and(|current| same(current, owner))
        {
            return Err(AdapterError::Rejected(
                "fixture desktop authority lost".into(),
            ));
        }
        state.frames += 1;
        Ok(())
    }
}

#[async_trait::async_trait]
impl DesktopAuthorization for Ownership {
    async fn current(&self, caller: &Caller) -> Result<(), DesktopError> {
        self.checks.fetch_add(1, Ordering::SeqCst);
        if caller.subject.as_str() != self.subject {
            return Err(DesktopError::Unavailable);
        }
        if self.revoked.load(Ordering::SeqCst) {
            Err(DesktopError::AuthorityUnavailable)
        } else {
            Ok(())
        }
    }
}

#[async_trait::async_trait]
impl DesktopOwnershipControl for Ownership {
    async fn ownership_challenge(&self) -> layer3_adapter::Result<DesktopOwnershipChallenge> {
        Ok(self.challenge())
    }
    async fn renew_ownership(
        &self,
        owner: &DesktopOwnership,
        challenge: &DesktopOwnershipChallenge,
    ) -> layer3_adapter::Result<()> {
        self.confirm(owner, challenge);
        Ok(())
    }
    async fn revoke_ownership(&self, owner: &DesktopOwnership) -> layer3_adapter::Result<()> {
        let mut state = self.state.lock().unwrap();
        if state
            .receiver
            .as_ref()
            .is_some_and(|current| same(current, owner))
        {
            state.receiver = None;
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl SandboxSessionStore for Ownership {
    async fn acquire_exclusive_sandbox_session(
        &self,
        request: &DesktopSessionRequest,
    ) -> Result<DesktopOwnership, SandboxStoreError> {
        let mut state = self.state.lock().unwrap();
        if state.claim.is_some() {
            return Err(SandboxStoreError::Conflict);
        }
        state.epoch += 1;
        let owner = DesktopOwnership {
            sandbox_id: request.sandbox_id.to_string(),
            session_id: request.session_id.clone(),
            runtime_id: request.runtime.runtime_id.clone(),
            boot_id: request.runtime.boot_id.clone(),
            epoch: state.epoch,
            issued_at_ms: 1,
            expires_at_ms: 15_001,
        };
        state.claim = Some(owner.clone());
        state.writes += 1;
        Ok(owner)
    }
    async fn renew_exclusive_sandbox_session(
        &self,
        request: &DesktopSessionRequest,
        owner: &DesktopOwnership,
    ) -> Result<DesktopOwnership, SandboxStoreError> {
        assert_eq!(request.session_id, owner.session_id);
        assert_eq!(request.sandbox_id.as_str(), owner.sandbox_id);
        let mut state = self.state.lock().unwrap();
        if state.claim.as_ref() != Some(owner) {
            return Err(SandboxStoreError::Conflict);
        }
        let mut next = owner.clone();
        next.issued_at_ms += 3_000;
        next.expires_at_ms += 3_000;
        state.claim = Some(next.clone());
        state.writes += 1;
        Ok(next)
    }
    async fn release_exclusive_sandbox_session(
        &self,
        owner: &DesktopOwnership,
    ) -> Result<(), SandboxStoreError> {
        let mut state = self.state.lock().unwrap();
        if state
            .claim
            .as_ref()
            .is_some_and(|current| same(current, owner))
        {
            assert!(
                state.receiver.is_none(),
                "receiver must be revoked before releasing its claim"
            );
            state.claim = None;
            state.releases += 1;
        }
        Ok(())
    }
    async fn has_open_sandbox_session(
        &self,
        id: &SandboxId,
        _: &str,
    ) -> Result<bool, SandboxStoreError> {
        Ok(self
            .state
            .lock()
            .unwrap()
            .claim
            .as_ref()
            .is_some_and(|owner| owner.sandbox_id == id.as_str()))
    }
    async fn open_sandbox_session(
        &self,
        _: &SandboxId,
        _: &str,
        _: &str,
    ) -> Result<(), SandboxStoreError> {
        panic!("new public desktop must use protected admission")
    }
    async fn renew_sandbox_session(
        &self,
        _: &SandboxId,
        _: &str,
        _: &str,
    ) -> Result<(), SandboxStoreError> {
        panic!("new public desktop must use protected renewal")
    }
    async fn close_sandbox_session(&self, _: &SandboxId, _: &str) -> Result<(), SandboxStoreError> {
        panic!("new public desktop must use exact protected release")
    }
}
