use super::*;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Mutex,
};

pub(crate) struct TestAuthority {
    pub owner: TransportOwner,
    pub renewals: AtomicUsize,
    pub fail_renewal: AtomicBool,
    pub events: Arc<Mutex<Vec<&'static str>>>,
    pub confirmed_bindings: Mutex<Vec<String>>,
    pub failure: std::sync::OnceLock<OwnershipFailure>,
    pub diagnostics_enabled: AtomicBool,
    pub diagnostic_gate: Mutex<Option<Arc<tokio::sync::Notify>>>,
    pub diagnostic_entered: tokio::sync::Notify,
    pub failure_before_revocation: AtomicBool,
    pub receivers: Mutex<Vec<Arc<dyn ReceiverRevocation>>>,
    revoked: tokio::sync::watch::Sender<bool>,
}

impl TestAuthority {
    pub fn new(epoch: u64) -> Arc<Self> {
        Arc::new(Self {
            owner: TransportOwner {
                protocol: OWNER_PROTOCOL_VERSION,
                agent_id: "agent-1".into(),
                instance_id: format!("member-{epoch}"),
                epoch,
            },
            renewals: AtomicUsize::new(0),
            fail_renewal: AtomicBool::new(false),
            events: Arc::default(),
            confirmed_bindings: Mutex::default(),
            failure: Default::default(),
            diagnostics_enabled: AtomicBool::new(true),
            diagnostic_gate: Mutex::new(None),
            diagnostic_entered: tokio::sync::Notify::new(),
            failure_before_revocation: AtomicBool::new(false),
            receivers: Mutex::default(),
            revoked: tokio::sync::watch::channel(false).0,
        })
    }
}

#[async_trait::async_trait]
impl TransportAuthority for TestAuthority {
    fn register_receiver(&self, receiver: Arc<dyn ReceiverRevocation>) -> crate::Result<()> {
        self.receivers.lock().unwrap().push(receiver);
        Ok(())
    }
    fn receiver_confirmed(&self, identity: &str) {
        self.confirmed_bindings
            .lock()
            .unwrap()
            .push(identity.into());
    }
    fn begin_receiver_confirmation(&self, identity: &str) -> crate::Result<()> {
        if self.is_current()
            && self
                .receivers
                .lock()
                .unwrap()
                .iter()
                .any(|receiver| receiver.identity() == identity)
        {
            Ok(())
        } else {
            Err(crate::AdapterError::Rejected(
                "missing receiver custody".into(),
            ))
        }
    }
    fn is_current(&self) -> bool {
        !*self.revoked.borrow()
    }
    fn owner(&self) -> TransportOwner {
        self.owner.clone()
    }
    async fn check(&self) -> crate::Result<()> {
        if *self.revoked.borrow() {
            Err(crate::AdapterError::Rejected("revoked".into()))
        } else {
            Ok(())
        }
    }
    async fn renew_for_challenge(&self) -> crate::Result<()> {
        self.check().await?;
        if self.fail_renewal.load(Ordering::SeqCst) {
            return Err(crate::AdapterError::Unavailable("partition".into()));
        }
        self.events.lock().unwrap().push("renew");
        self.renewals.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn revoked(&self) {
        let _ = self.revoked.subscribe().wait_for(|revoked| *revoked).await;
    }
    async fn record_failure(&self, failure: OwnershipFailure) {
        if !self.diagnostics_enabled.load(Ordering::SeqCst) {
            return;
        }
        self.failure.get_or_init(|| {
            self.failure_before_revocation
                .store(self.is_current(), Ordering::SeqCst);
            failure
        });
        let gate = self.diagnostic_gate.lock().unwrap().clone();
        if let Some(gate) = gate {
            self.diagnostic_entered.notify_one();
            gate.notified().await;
        }
    }
    fn revoke(&self) {
        self.revoked.send_replace(true);
    }
}
