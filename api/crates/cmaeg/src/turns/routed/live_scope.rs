//! A stage owns renewal of one captured generation; snapshots never adopt peers.
use super::{
    tool_reply_store::ReplyClaim, ExecutionError, Journal, Lease, MediaLease, MediaObligationStore,
};
use crate::replicas::{config::RENEW, partition::PartitionLease};
use std::{
    future::Future,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, RwLock,
    },
};
use tokio::{sync::Mutex, time::Duration};

#[derive(Clone)]
pub(super) enum Capture {
    Execution(Box<Lease>, Option<Box<ReplyClaim>>),
    Reply(Box<ReplyClaim>),
    Effect(Box<PartitionLease>),
    Media(Box<MediaLease>),
}
impl Capture {
    pub fn execution(lease: Lease, reply: Option<ReplyClaim>) -> Self {
        Self::Execution(Box::new(lease), reply.map(Box::new))
    }
    pub fn reply(claim: ReplyClaim) -> Self {
        Self::Reply(Box::new(claim))
    }
    pub fn effect(token: PartitionLease) -> Self {
        Self::Effect(Box::new(token))
    }
    pub fn media(lease: MediaLease) -> Self {
        Self::Media(Box::new(lease))
    }

    fn partition(&self) -> Option<&PartitionLease> {
        match self {
            Self::Execution(lease, _) => lease.partition.as_ref(),
            Self::Reply(claim) => claim.partition.as_ref(),
            Self::Effect(token) => Some(token),
            Self::Media(lease) => lease.partition.as_ref(),
        }
    }
}
pub(super) struct Scope {
    journal: Journal,
    capture: RwLock<Capture>,
    serial: Mutex<()>,
    cancelled: AtomicBool,
}
impl Scope {
    pub fn new(journal: Journal, capture: Capture) -> Arc<Self> {
        Arc::new(Self {
            journal,
            capture: RwLock::new(capture),
            serial: Mutex::new(()),
            cancelled: AtomicBool::new(false),
        })
    }
    pub fn capture(&self) -> Capture {
        self.capture.read().unwrap().clone()
    }
    pub fn live(&self) -> bool {
        !self.cancelled.load(Ordering::SeqCst)
            && self
                .capture
                .read()
                .unwrap()
                .partition()
                .is_none_or(PartitionLease::live)
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }
    pub fn execution(&self) -> Lease {
        let Capture::Execution(lease, _) = self.capture() else {
            unreachable!()
        };
        *lease
    }
    pub fn reply(&self) -> ReplyClaim {
        match self.capture() {
            Capture::Execution(_, Some(claim)) | Capture::Reply(claim) => *claim,
            _ => unreachable!(),
        }
    }
    pub fn effect(&self) -> PartitionLease {
        let Capture::Effect(token) = self.capture() else {
            unreachable!()
        };
        *token
    }
    pub fn media(&self) -> MediaLease {
        let Capture::Media(lease) = self.capture() else {
            unreachable!()
        };
        *lease
    }
    pub async fn guard(&self) -> Result<(), ExecutionError> {
        let _lock = self.serial.lock().await;
        if !self.live() {
            return Err(ExecutionError::LostLease);
        }
        match self.capture() {
            Capture::Execution(lease, reply) => {
                self.journal.guard(&lease).await?;
                if let Some(claim) = reply {
                    super::tool_reply_store::ReplyStore(self.journal.clone(), lease.partition)
                        .guard(&claim)
                        .await?;
                }
            }
            Capture::Reply(claim) => {
                super::tool_reply_store::ReplyStore(self.journal.clone(), claim.partition.clone())
                    .guard(&claim)
                    .await?
            }
            Capture::Effect(token) => self.journal.guard_partition(&Some(*token)).await?,
            Capture::Media(lease) => {
                MediaObligationStore(self.journal.clone())
                    .guard(&lease)
                    .await?
            }
        }
        if !self.live() {
            return Err(ExecutionError::LostLease);
        }
        Ok(())
    }
    async fn renew(&self) -> Result<(), ExecutionError> {
        let _lock = self.serial.lock().await;
        if !self.live() {
            return Err(ExecutionError::LostLease);
        }
        let mut next = self.capture();
        match &mut next {
            Capture::Execution(lease, reply) => {
                self.journal.renew(lease).await?;
                if let Some(claim) = reply {
                    claim.partition = lease.partition.clone();
                    super::tool_reply_store::ReplyStore(
                        self.journal.clone(),
                        lease.partition.clone(),
                    )
                    .renew(claim)
                    .await?;
                }
            }
            Capture::Reply(claim) => {
                if let Some(token) = &mut claim.partition {
                    self.journal.renew_partition(token).await?;
                }
                super::tool_reply_store::ReplyStore(self.journal.clone(), claim.partition.clone())
                    .renew(claim)
                    .await?;
            }
            Capture::Effect(token) => self.journal.renew_partition(token).await?,
            Capture::Media(lease) => {
                MediaObligationStore(self.journal.clone())
                    .renew(lease)
                    .await?
            }
        }
        if self.cancelled.load(Ordering::SeqCst) {
            return Err(ExecutionError::LostLease);
        }
        *self.capture.write().unwrap() = next;
        Ok(())
    }
    /// No task is spawned. Cancelling this future drops renewal and cancels gates.
    pub async fn run<T>(
        &self,
        stage: &'static str,
        future: impl Future<Output = T>,
    ) -> Result<T, ExecutionError> {
        struct Cancel<'a>(&'a Scope);
        impl Drop for Cancel<'_> {
            fn drop(&mut self) {
                self.0.cancel();
            }
        }
        let _cancel = Cancel(self);
        if self.capture().partition().is_none() {
            return Ok(future.await);
        }
        tokio::pin!(future);
        let run = async {
            let deadline = self.capture.read().unwrap().partition().unwrap().deadline();
            tokio::time::timeout_at(deadline, self.renew())
                .await
                .map_err(|_| ExecutionError::LostLease)??;
            loop {
                tokio::select! {
                    biased;
                    _ = tokio::time::sleep(RENEW) => {},
                    value = &mut future => {
                        if !self.live() { return Err(ExecutionError::LostLease); }
                        return Ok(value);
                    }
                }
                let deadline = self.capture.read().unwrap().partition().unwrap().deadline();
                let renewal = tokio::time::timeout_at(deadline, self.renew());
                tokio::pin!(renewal);
                tokio::select! {
                    biased;
                    result = &mut renewal => { result.map_err(|_| ExecutionError::LostLease)??; },
                    value = &mut future => {
                        // Do not cancel an in-flight CAS on a healthy receipt.
                        // Finish its exact generation before publishing the snapshot.
                        renewal.await.map_err(|_| ExecutionError::LostLease)??;
                        if !self.live() { return Err(ExecutionError::LostLease); }
                        return Ok(value);
                    }
                }
            }
        }
        .await;
        if run.is_err() {
            self.cancel();
            metrics::counter!("cmaeg_replica_stage_loss_total", "stage" => stage).increment(1);
            tracing::warn!(
                stage,
                cause = "renewal_failed",
                "replica stage cancelled; retained evidence requires recovery"
            );
        }
        run
    }
}
/// Send windows remain 20s; a separately bounded receipt commit has five seconds.
/// The refreshed 30s guard claim retains at least ten seconds at send completion.
pub(super) const RECEIPT_BUDGET: Duration = Duration::from_secs(5);
pub(super) async fn receipt<T>(
    future: impl Future<Output = Result<T, ExecutionError>>,
) -> Result<T, ExecutionError> {
    tokio::time::timeout(RECEIPT_BUDGET, future)
        .await
        .map_err(|_| ExecutionError::Unavailable)?
}
