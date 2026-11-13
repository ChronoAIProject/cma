//! Shared receiver fixture models one runtime, not shared AM memory.
use layer3_adapter::*;
use mongodb::{bson::doc, Database};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    sync::{mpsc, Mutex, Notify, Semaphore},
    time::Instant,
};

pub(super) struct Receiver {
    db: Database,
    state: Arc<State>,
    pub(super) fail_open: AtomicU8,
    pub(super) legacy_opens: AtomicUsize,
}
struct State {
    challenges: Mutex<HashMap<String, Instant>>,
    active: Mutex<Option<(DesktopOwnership, Instant)>>,
    pub(super) frames: AtomicUsize,
    block_renew: AtomicBool,
    renewing: Notify,
    resume: Semaphore,
}

impl Receiver {
    pub(super) fn new(db: Database) -> Self {
        Self {
            db,
            state: Arc::new(State {
                challenges: Mutex::new(HashMap::new()),
                active: Mutex::new(None),
                frames: AtomicUsize::new(0),
                block_renew: AtomicBool::new(false),
                renewing: Notify::new(),
                resume: Semaphore::new(0),
            }),
            fail_open: AtomicU8::new(0),
            legacy_opens: AtomicUsize::new(0),
        }
    }
    pub(super) fn block_renew(&self) {
        self.state.block_renew.store(true, Ordering::SeqCst);
    }
    pub(super) async fn renewing(&self) {
        self.state.renewing.notified().await;
    }
    pub(super) fn frames(&self) -> usize {
        self.state.frames.load(Ordering::SeqCst)
    }
    pub(super) async fn active_claim(&self) -> DesktopOwnership {
        self.state.active.lock().await.as_ref().unwrap().0.clone()
    }
    async fn committed(&self, claim: &DesktopOwnership) {
        let row = self
            .db
            .collection::<mongodb::bson::Document>("sandbox_sessions")
            .find_one(
                doc! { "_id": &claim.session_id, "epoch": claim.epoch as i64,
                "expires_at_ms": claim.expires_at_ms },
            )
            .await
            .unwrap();
        assert!(
            row.is_some(),
            "provider open/confirm must follow Mongo commit"
        );
    }
}
impl State {
    async fn challenge(&self) -> DesktopOwnershipChallenge {
        let nonce = uuid::Uuid::new_v4().to_string();
        self.challenges
            .lock()
            .await
            .insert(nonce.clone(), Instant::now() + Duration::from_secs(10));
        DesktopOwnershipChallenge {
            nonce,
            valid_for_ms: 10_000,
        }
    }
    async fn install(
        &self,
        claim: &DesktopOwnership,
        challenge: &DesktopOwnershipChallenge,
    ) -> Result<()> {
        claim.validate()?;
        challenge.validate()?;
        let deadline = self
            .challenges
            .lock()
            .await
            .remove(&challenge.nonce)
            .unwrap();
        assert!(deadline > Instant::now());
        let mut active = self.active.lock().await;
        if let Some((previous, _)) = &*active {
            assert!(claim.epoch >= previous.epoch);
            if previous.epoch == claim.epoch {
                assert_eq!(claim.session_id, previous.session_id);
            }
        }
        *active = Some((claim.clone(), deadline));
        Ok(())
    }
}

struct Control {
    receiver: Arc<Receiver>,
}
#[async_trait::async_trait]
impl DesktopOwnershipControl for Control {
    async fn ownership_challenge(&self) -> Result<DesktopOwnershipChallenge> {
        if self.receiver.state.block_renew.load(Ordering::SeqCst) {
            self.receiver.state.renewing.notify_one();
            self.receiver.state.resume.acquire().await.unwrap().forget();
        }
        Ok(self.receiver.state.challenge().await)
    }
    async fn renew_ownership(
        &self,
        claim: &DesktopOwnership,
        challenge: &DesktopOwnershipChallenge,
    ) -> Result<()> {
        self.receiver.committed(claim).await;
        self.receiver.state.install(claim, challenge).await
    }
    async fn revoke_ownership(&self, claim: &DesktopOwnership) -> Result<()> {
        let mut active = self.receiver.state.active.lock().await;
        if active.as_ref().is_some_and(|(current, _)| {
            current.epoch == claim.epoch && current.session_id == claim.session_id
        }) {
            *active = None;
        }
        Ok(())
    }
}
struct Socket {
    state: Arc<State>,
    control: Option<Arc<dyn DesktopOwnershipControl>>,
    epoch: Option<u64>,
}
#[async_trait::async_trait]
impl DesktopConnection for Socket {
    async fn recv(&mut self) -> Result<Option<DesktopFrame>> {
        std::future::pending().await
    }
    async fn send(&mut self, _: DesktopFrame) -> Result<()> {
        if let Some(epoch) = self.epoch {
            if !self
                .state
                .active
                .lock()
                .await
                .as_ref()
                .is_some_and(|(claim, deadline)| claim.epoch == epoch && *deadline > Instant::now())
            {
                return Err(AdapterError::Rejected("expired desktop claim".into()));
            }
        }
        self.state.frames.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn ownership_control(&self) -> Option<Arc<dyn DesktopOwnershipControl>> {
        self.control.clone()
    }
}

#[async_trait::async_trait]
impl SandboxAdapter for Receiver {
    async fn desktop_ownership_challenge(
        &self,
        _: &RuntimeHandle,
    ) -> Result<DesktopOwnershipChallenge> {
        Ok(self.state.challenge().await)
    }
    async fn open_desktop_owned(
        &self,
        _: &RuntimeHandle,
        claim: &DesktopOwnership,
        challenge: &DesktopOwnershipChallenge,
    ) -> Result<DesktopConnectionHandle> {
        self.committed(claim).await;
        match self.fail_open.load(Ordering::SeqCst) {
            1 => {
                return Err(AdapterError::Unsupported(
                    "fixture receiver unsupported".into(),
                ))
            }
            2 => return Err(AdapterError::Rejected("fixture uncertain open".into())),
            _ => {}
        }
        self.state.install(claim, challenge).await?;
        Ok(Box::new(Socket {
            state: self.state.clone(),
            epoch: Some(claim.epoch),
            control: Some(Arc::new(Control {
                receiver: Arc::new(Receiver {
                    db: self.db.clone(),
                    state: self.state.clone(),
                    fail_open: AtomicU8::new(0),
                    legacy_opens: AtomicUsize::new(0),
                }),
            })),
        }))
    }
    async fn open_desktop(&self, _: &RuntimeHandle) -> Result<DesktopConnectionHandle> {
        self.legacy_opens.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(Socket {
            state: self.state.clone(),
            control: None,
            epoch: None,
        }))
    }
    fn capabilities(&self) -> AdapterCapabilities {
        panic!("unexpected runtime effect")
    }
    async fn provision(&self, _spec: &SandboxSpec) -> Result<RuntimeHandle> {
        panic!("unexpected runtime effect")
    }
    async fn run_install(
        &self,
        _handle: &RuntimeHandle,
        _request: &InstallRequest,
    ) -> Result<InstallReceipt> {
        panic!("unexpected runtime effect")
    }
    async fn deliver_credential(
        &self,
        _handle: &RuntimeHandle,
        _frame: CredentialFrame,
    ) -> Result<()> {
        panic!("unexpected runtime effect")
    }
    async fn prepare_workspace(
        &self,
        _handle: &RuntimeHandle,
        _request: &WorkspaceRequest,
    ) -> Result<WorkspaceReceipt> {
        panic!("unexpected runtime effect")
    }
    async fn run_workspace_setup(
        &self,
        _handle: &RuntimeHandle,
        _request: &WorkspaceSetupRequest,
    ) -> Result<SetupReceipt> {
        panic!("unexpected runtime effect")
    }
    async fn remove_workspace(
        &self,
        _handle: &RuntimeHandle,
        _request: &WorkspaceRemoveRequest,
    ) -> Result<()> {
        panic!("unexpected runtime effect")
    }
    async fn add_worktree(
        &self,
        _handle: &RuntimeHandle,
        _authority_fence: u64,
        _request: &WorktreeRequest,
    ) -> Result<WorktreeReceipt> {
        panic!("unexpected runtime effect")
    }
    async fn remove_worktree(
        &self,
        _handle: &RuntimeHandle,
        _authority_fence: u64,
        _request: &WorktreeRef,
    ) -> Result<()> {
        panic!("unexpected runtime effect")
    }
    async fn materialize_agent_home(
        &self,
        _handle: &RuntimeHandle,
        _authority_fence: u64,
        _request: &AgentHomeRequest,
    ) -> Result<()> {
        panic!("unexpected runtime effect")
    }
    async fn remove_agent_home(
        &self,
        _handle: &RuntimeHandle,
        _authority_fence: u64,
        _request: &AgentHomeRemoveRequest,
    ) -> Result<()> {
        panic!("unexpected runtime effect")
    }
    async fn open_agent(
        &self,
        _handle: &RuntimeHandle,
        _authority_fence: u64,
        _request: &AgentOpenRequest,
        _updates: mpsc::Sender<ConversationUpdate>,
    ) -> Result<ConversationChannel> {
        panic!("unexpected runtime effect")
    }
    async fn reconnect_agent(
        &self,
        _handle: &RuntimeHandle,
        _authority_fence: u64,
        _request: &AgentOpenRequest,
        _conversation_id: &str,
        _updates: mpsc::Sender<ConversationUpdate>,
    ) -> Result<ConversationChannel> {
        panic!("unexpected runtime effect")
    }
    async fn close_agent(
        &self,
        _handle: &RuntimeHandle,
        _authority_fence: u64,
        _request: &AgentCloseRequest,
    ) -> Result<()> {
        panic!("unexpected runtime effect")
    }
    async fn capture_agent(
        &self,
        _handle: &RuntimeHandle,
        _agent: &AgentSlot,
    ) -> Result<ConversationSnapshot> {
        panic!("unexpected runtime effect")
    }
    async fn checkpoint(
        &self,
        _handle: &RuntimeHandle,
        _request: &CheckpointRequest,
    ) -> Result<CheckpointReceipt> {
        panic!("unexpected runtime effect")
    }
    async fn restore(&self, _handle: &RuntimeHandle, _request: &RestoreRequest) -> Result<()> {
        panic!("unexpected runtime effect")
    }
    async fn resources(&self, _handle: &RuntimeHandle) -> Result<ResourceReading> {
        panic!("unexpected runtime effect")
    }
    async fn teardown(&self, _handle: &RuntimeHandle) -> Result<()> {
        panic!("unexpected runtime effect")
    }
}
