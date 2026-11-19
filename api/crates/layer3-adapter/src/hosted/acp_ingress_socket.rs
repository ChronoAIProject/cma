//! Adapt legacy and admitted sockets to the Bridge transport boundary.

use crate::ingress_budget::{
    AdmittedSocket, IngressOwnershipPolicy, IngressWorkScope, SharedOwned,
};
use futures_util::{Sink, Stream};
use std::{
    ops::Deref,
    pin::Pin,
    task::{Context, Poll},
};
use tokio_tungstenite::tungstenite::{Error, Message};

#[path = "acp_ingress_split.rs"]
mod split;
pub(in crate::hosted::acp) use split::{BridgeSink, BridgeSource};

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

pub(in crate::hosted::acp) enum OutgoingFrame {
    Legacy(Message),
    Admitted(SharedOwned<Message>),
}

impl OutgoingFrame {
    pub(super) fn prepare(
        message: Message,
        policy: Option<&IngressOwnershipPolicy>,
    ) -> Result<Self, &'static str> {
        let Some(policy) = policy else {
            return Ok(Self::Legacy(message));
        };
        let scope = IngressWorkScope::current().ok_or("missing_write_scope")?;
        if !scope.belongs_to(policy) {
            return Err("foreign_write_root");
        }
        Ok(Self::Admitted(scope.initialize(|| message)))
    }
}

pub(in crate::hosted::acp) type OwnedSocket = Box<dyn AdmittedSocket<Message>>;

pub(in crate::hosted::acp) enum BridgeSocket {
    Legacy(crate::hosted::acp::Socket),
    Admitted {
        socket: OwnedSocket,
        policy: IngressOwnershipPolicy,
    },
}

pub(in crate::hosted::acp) fn legacy(socket: crate::hosted::acp::Socket) -> BridgeSocket {
    BridgeSocket::Legacy(socket)
}

pub(in crate::hosted::acp) fn admitted(
    socket: OwnedSocket,
    policy: IngressOwnershipPolicy,
) -> BridgeSocket {
    BridgeSocket::Admitted { socket, policy }
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

impl Stream for BridgeSocket {
    type Item = Result<IncomingFrame, Error>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let (socket, policy) = match &mut *self {
            Self::Legacy(socket) => {
                return match Pin::new(socket).poll_next(cx) {
                    Poll::Ready(Some(Ok(frame))) => {
                        Poll::Ready(Some(Ok(IncomingFrame::Legacy(frame))))
                    }
                    Poll::Ready(Some(Err(error))) => Poll::Ready(Some(Err(error))),
                    Poll::Ready(None) => Poll::Ready(None),
                    Poll::Pending => Poll::Pending,
                }
            }
            Self::Admitted { socket, policy } => (socket, policy),
        };
        match Pin::new(socket).poll_next(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Ready(Some(Ok(frame))) if frame.belongs_to(policy) => {
                Poll::Ready(Some(Ok(IncomingFrame::Admitted(frame))))
            }
            Poll::Ready(Some(Ok(_))) => {
                Poll::Ready(Some(Err(refused_transport("foreign_frame_root"))))
            }
            Poll::Ready(Some(Err(error))) => Poll::Ready(Some(Err(refused(error)))),
        }
    }
}

impl Sink<OutgoingFrame> for BridgeSocket {
    type Error = Error;
    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        match &mut *self {
            Self::Legacy(socket) => Pin::new(socket).poll_ready(cx),
            Self::Admitted { socket, .. } => Pin::new(socket).poll_ready(cx).map_err(refused),
        }
    }
    fn start_send(mut self: Pin<&mut Self>, message: OutgoingFrame) -> Result<(), Error> {
        match (&mut *self, message) {
            (Self::Legacy(socket), OutgoingFrame::Legacy(message)) => {
                Pin::new(socket).start_send(message)
            }
            (Self::Admitted { socket, policy }, OutgoingFrame::Admitted(message))
                if message.belongs_to(policy) =>
            {
                Pin::new(socket).start_send(message).map_err(refused)
            }
            _ => Err(refused_transport("outgoing_frame_ownership")),
        }
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        match &mut *self {
            Self::Legacy(socket) => Pin::new(socket).poll_flush(cx),
            Self::Admitted { socket, .. } => Pin::new(socket).poll_flush(cx).map_err(refused),
        }
    }
    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        match &mut *self {
            Self::Legacy(socket) => Pin::new(socket).poll_close(cx),
            Self::Admitted { socket, .. } => Pin::new(socket).poll_close(cx).map_err(refused),
        }
    }
}
