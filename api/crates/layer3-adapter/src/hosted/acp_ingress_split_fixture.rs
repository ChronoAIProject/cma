use super::*;
use crate::ingress_budget::{ClassBounds, IngressBudget};
use std::sync::{
    atomic::{AtomicBool, Ordering::SeqCst},
    Arc, Mutex,
};

pub(super) struct ReadBlock {
    pub(super) entered: AtomicBool,
    pub(super) release: AtomicBool,
}

pub(super) struct Gate<I, E> {
    pub(super) ready: Arc<AtomicBool>,
    pub(super) sent: Arc<Mutex<Vec<I>>>,
    pub(super) read_block: Option<Arc<ReadBlock>>,
    error: std::marker::PhantomData<fn() -> E>,
}

impl<I, E> Gate<I, E> {
    pub(super) fn new() -> Self {
        let sent = Arc::new(Mutex::new(Vec::with_capacity(1)));
        drop(sent.lock().unwrap());
        Self {
            ready: Arc::new(AtomicBool::new(false)),
            sent,
            read_block: None,
            error: std::marker::PhantomData,
        }
    }
}

impl<I, E> Stream for Gate<I, E> {
    type Item = Result<I, E>;
    fn poll_next(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if let Some(block) = &self.read_block {
            block.entered.store(true, SeqCst);
            while !block.release.load(SeqCst) {
                std::thread::yield_now();
            }
        }
        Poll::Pending
    }
}

impl<I, E> Sink<I> for Gate<I, E> {
    type Error = E;
    fn poll_ready(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), E>> {
        if self.ready.load(SeqCst) {
            Poll::Ready(Ok(()))
        } else {
            Poll::Pending
        }
    }
    fn start_send(self: Pin<&mut Self>, value: I) -> Result<(), E> {
        self.sent.lock().unwrap().push(value);
        Ok(())
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), E>> {
        Poll::Ready(Ok(()))
    }
    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), E>> {
        Poll::Ready(Ok(()))
    }
}

pub(super) fn policy() -> (IngressBudget, IngressOwnershipPolicy) {
    let root = IngressBudget::new(64 * 1024);
    let floor = AllocationPlan::checked("split fixture floor", &[8192]).unwrap();
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(root.capacity_bytes(), floor, floor).unwrap(),
    )
    .unwrap();
    drop(policy.admit(IngressClass::Data, floor).unwrap());
    (root, policy)
}

pub(super) fn frame_scope(policy: &IngressOwnershipPolicy) -> IngressWorkScope {
    IngressWorkScope::new(
        policy
            .admit(
                IngressClass::Data,
                AllocationPlan::checked(
                    "queued split frame",
                    &[4096, sizing::shared_owner_bytes::<Message>()],
                )
                .unwrap(),
            )
            .unwrap(),
    )
}
