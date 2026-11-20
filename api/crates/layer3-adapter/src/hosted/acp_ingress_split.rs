//! Own the stock split allocation and every queued outgoing frame through drop.

use super::*;
use crate::ingress_budget::{sizing, AdmissionRefusal, AllocationPlan, IngressClass};
use futures_util::{
    stream::{SplitSink, SplitStream},
    StreamExt,
};

pub(in crate::hosted::acp) struct BridgeSink {
    inner: SplitSink<BridgeSocket, OutgoingFrame>,
    policy: Option<IngressOwnershipPolicy>,
    // Both halves drop their BiLock handles before their last custody handle.
    _storage: Option<IngressWorkScope>,
}

pub(in crate::hosted::acp) struct BridgeSource {
    inner: SplitStream<BridgeSocket>,
    _storage: Option<IngressWorkScope>,
}

impl BridgeSocket {
    pub(in crate::hosted::acp) fn split(
        self,
    ) -> Result<(BridgeSink, BridgeSource), AdmissionRefusal> {
        let policy = match &self {
            Self::Legacy(_) => None,
            Self::Admitted { policy, .. } => Some(policy.clone()),
        };
        let storage = policy
            .as_ref()
            .map(|policy| {
                policy
                    .admit(IngressClass::Data, split_plan()?)
                    .map(IngressWorkScope::new)
            })
            .transpose()?;
        let (sink, source) = StreamExt::split::<OutgoingFrame>(self);
        Ok((
            BridgeSink {
                inner: sink,
                policy,
                _storage: storage.clone(),
            },
            BridgeSource {
                inner: source,
                _storage: storage,
            },
        ))
    }
}

fn split_plan() -> Result<AllocationPlan, AdmissionRefusal> {
    // futures-util 0.3.32 BiLock stores AtomicPtr<Waker> and
    // Option<UnsafeCell<Socket>> in one Arc. Include field/Arc alignment and
    // both a parked Waker box and its overlapping replacement while unlocking.
    // The pinned Tokio/noop waker clone increments a reference without boxing.
    AllocationPlan::checked(
        "bridge socket split",
        &[
            size_of::<Option<std::cell::UnsafeCell<BridgeSocket>>>(),
            size_of::<std::sync::atomic::AtomicPtr<std::task::Waker>>(),
            2 * size_of::<usize>(),
            2 * align_of::<BridgeSocket>(),
            2 * size_of::<std::task::Waker>(),
            sizing::shared_owner_bytes::<()>(),
        ],
    )
}

impl Stream for BridgeSource {
    type Item = Result<IncomingFrame, Error>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.inner).poll_next(cx)
    }
}

impl Sink<Message> for BridgeSink {
    type Error = Error;
    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        Pin::new(&mut self.inner).poll_ready(cx)
    }
    fn start_send(mut self: Pin<&mut Self>, message: Message) -> Result<(), Error> {
        // SplitSink may keep this slot after the initiating send is cancelled.
        // Attach custody before the slot is filled, not on its later flush.
        let frame =
            OutgoingFrame::prepare(message, self.policy.as_ref()).map_err(refused_transport)?;
        Pin::new(&mut self.inner).start_send(frame)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        Pin::new(&mut self.inner).poll_close(cx)
    }
}

#[cfg(test)]
#[path = "acp_ingress_split_tests.rs"]
mod tests;
