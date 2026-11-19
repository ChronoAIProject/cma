//! Adapt legacy and admitted sockets to the Bridge transport boundary.

use crate::ingress_budget::{AdmittedSocket, IngressWorkScope, SharedOwned};
use futures_util::{Sink, Stream, StreamExt};
use std::{
    ops::Deref,
    pin::Pin,
    task::{Context, Poll},
};
use tokio_tungstenite::tungstenite::{Error, Message};

pub(in crate::hosted::acp) enum IncomingFrame {
    Legacy(Message),
    Admitted(SharedOwned<Message>),
}

impl Deref for IncomingFrame {
    type Target = Message;
    fn deref(&self) -> &Message {
        match self {
            Self::Legacy(message) => message,
            Self::Admitted(message) => message,
        }
    }
}

pub(in crate::hosted::acp) trait BridgeIo:
    Stream<Item = Result<IncomingFrame, Error>> + Sink<Message, Error = Error> + Unpin + Send
{
}
impl<T> BridgeIo for T where
    T: Stream<Item = Result<IncomingFrame, Error>> + Sink<Message, Error = Error> + Unpin + Send
{
}

pub(in crate::hosted::acp) type BridgeSocket = Box<dyn BridgeIo>;
pub(in crate::hosted::acp) type OwnedSocket = Box<dyn AdmittedSocket<Message>>;

#[expect(
    clippy::result_large_err,
    reason = "preserve the existing socket error without allocating another box"
)]
pub(in crate::hosted::acp) fn legacy(socket: crate::hosted::acp::Socket) -> BridgeSocket {
    Box::new(socket.map(|frame| frame.map(IncomingFrame::Legacy)))
}

pub(in crate::hosted::acp) fn admitted(
    socket: OwnedSocket,
    policy: crate::ingress_budget::IngressOwnershipPolicy,
) -> BridgeSocket {
    Box::new(AdmittedIo { socket, policy })
}

struct AdmittedIo {
    socket: OwnedSocket,
    policy: crate::ingress_budget::IngressOwnershipPolicy,
}

fn refused(_: crate::ingress_budget::AdmittedError) -> Error {
    refused_transport("producer_refusal")
}

fn refused_transport(reason: &'static str) -> Error {
    tracing::debug!(reason, "admitted conversation transport refused progress");
    // A custom io::Error boxes its diagnostic before any operation scope exists.
    // Keep the same IO kind and emit only a static reason at this boundary.
    Error::Io(std::io::ErrorKind::Other.into())
}

impl Stream for AdmittedIo {
    type Item = Result<IncomingFrame, Error>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.socket).poll_next(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Ready(Some(Ok(frame))) if frame.belongs_to(&self.policy) => {
                Poll::Ready(Some(Ok(IncomingFrame::Admitted(frame))))
            }
            Poll::Ready(Some(Ok(_))) => {
                Poll::Ready(Some(Err(refused_transport("foreign_frame_root"))))
            }
            Poll::Ready(Some(Err(error))) => Poll::Ready(Some(Err(refused(error)))),
        }
    }
}

impl Sink<Message> for AdmittedIo {
    type Error = Error;
    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        Pin::new(&mut self.socket).poll_ready(cx).map_err(refused)
    }
    fn start_send(mut self: Pin<&mut Self>, message: Message) -> Result<(), Error> {
        let scope =
            IngressWorkScope::current().ok_or_else(|| refused_transport("missing_write_scope"))?;
        if !scope.belongs_to(&self.policy) {
            return Err(refused_transport("foreign_write_root"));
        }
        Pin::new(&mut self.socket)
            .start_send(scope.initialize(|| message))
            .map_err(refused)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        Pin::new(&mut self.socket).poll_flush(cx).map_err(refused)
    }
    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        Pin::new(&mut self.socket).poll_close(cx).map_err(refused)
    }
}
