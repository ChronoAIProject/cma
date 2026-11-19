//! Independent payload copies admitted before allocation.
//!
//! These functions own the destination. The caller must keep the codec source
//! and its backing allowance alive through the copy; they do not admit a read.

use tokio_tungstenite::tungstenite as ws024;
use tokio_tungstenite_029::tungstenite as ws029;

use crate::{
    ingress_budget::{
        sizing::shared_owner_bytes, AdmittedFrame, AllocationPlan, IngressClass,
        IngressOwnershipPolicy, ReservationBundle,
    },
    AdapterError, DesktopFrame, Result,
};

enum View<'a> {
    Binary(&'a [u8]),
    Text(&'a str),
    Ping(&'a [u8]),
    Pong(&'a [u8]),
    Close(Option<(u16, &'a str)>),
}

impl View<'_> {
    fn payload_bytes(&self) -> usize {
        match self {
            Self::Binary(bytes) | Self::Ping(bytes) | Self::Pong(bytes) => bytes.len(),
            Self::Text(text) => text.len(),
            Self::Close(Some((_, reason))) => reason.len(),
            Self::Close(None) => 0,
        }
    }

    fn reserve<T>(
        policy: &IngressOwnershipPolicy,
        payload_bytes: usize,
    ) -> Result<ReservationBundle> {
        // Every destination gets fresh storage; source Bytes/Vec backing is
        // never relabelled as though the copy reused its allocation.
        policy.reserve(
            IngressClass::Data,
            AllocationPlan::new(
                "transport_message_copy",
                &[payload_bytes, shared_owner_bytes::<T>()],
            )?,
        )
    }

    fn desktop(self, policy: &IngressOwnershipPolicy) -> Result<AdmittedFrame<DesktopFrame>> {
        // Desktop's existing semantic close carries no protocol reason/code.
        // The codec remains responsible for its protocol close response.
        let bytes = if matches!(self, Self::Close(_)) {
            0
        } else {
            self.payload_bytes()
        };
        let storage = Self::reserve::<DesktopFrame>(policy, bytes)?;
        Ok(storage.initialize(|| match self {
            Self::Binary(bytes) => DesktopFrame::Binary(bytes.to_vec()),
            Self::Text(text) => DesktopFrame::Text(text.to_owned()),
            Self::Ping(bytes) => DesktopFrame::Ping(bytes.to_vec()),
            Self::Pong(bytes) => DesktopFrame::Pong(bytes.to_vec()),
            Self::Close(_) => DesktopFrame::Close,
        }))
    }

    fn conversation(
        self,
        policy: &IngressOwnershipPolicy,
    ) -> Result<AdmittedFrame<ws024::Message>> {
        let storage = Self::reserve::<ws024::Message>(policy, self.payload_bytes())?;
        Ok(storage.initialize(|| match self {
            Self::Binary(bytes) => ws024::Message::Binary(bytes.to_vec()),
            Self::Text(text) => ws024::Message::Text(text.to_owned()),
            Self::Ping(bytes) => ws024::Message::Ping(bytes.to_vec()),
            Self::Pong(bytes) => ws024::Message::Pong(bytes.to_vec()),
            Self::Close(close) => {
                ws024::Message::Close(close.map(|(code, reason)| ws024::protocol::CloseFrame {
                    code: code.into(),
                    reason: reason.to_owned().into(),
                }))
            }
        }))
    }
}

fn view_024(source: &ws024::Message) -> Result<View<'_>> {
    Ok(match source {
        ws024::Message::Binary(bytes) => View::Binary(bytes),
        ws024::Message::Text(text) => View::Text(text),
        ws024::Message::Ping(bytes) => View::Ping(bytes),
        ws024::Message::Pong(bytes) => View::Pong(bytes),
        ws024::Message::Close(close) => View::Close(
            close
                .as_ref()
                .map(|close| (close.code.into(), close.reason.as_ref())),
        ),
        // Stock reads never emit Frame. Reject outbound raw frames rather than
        // silently losing opcode, mask, FIN or reserved-bit metadata.
        ws024::Message::Frame(_) => return Err(AdapterError::Unsupported(String::new())),
    })
}

fn view_029(source: &ws029::Message) -> Result<View<'_>> {
    Ok(match source {
        ws029::Message::Binary(bytes) => View::Binary(bytes),
        ws029::Message::Text(text) => View::Text(text.as_str()),
        ws029::Message::Ping(bytes) => View::Ping(bytes),
        ws029::Message::Pong(bytes) => View::Pong(bytes),
        ws029::Message::Close(close) => View::Close(
            close
                .as_ref()
                .map(|close| (close.code.into(), close.reason.as_str())),
        ),
        ws029::Message::Frame(_) => return Err(AdapterError::Unsupported(String::new())),
    })
}

/// Copy into desktop storage after reserving the full destination allocation.
/// Retain the source codec's allowance until this operation returns.
pub fn copy_desktop_024(
    policy: &IngressOwnershipPolicy,
    source: &ws024::Message,
) -> Result<AdmittedFrame<DesktopFrame>> {
    view_024(source)?.desktop(policy)
}

/// Copy shared codec backing into independently owned desktop storage.
/// A short source slice does not release its source backing allowance.
pub fn copy_desktop_029(
    policy: &IngressOwnershipPolicy,
    source: &ws029::Message,
) -> Result<AdmittedFrame<DesktopFrame>> {
    view_029(source)?.desktop(policy)
}

/// Copy a decoded message into the admitted conversation port's storage.
/// This preserves control kinds and close code/reason, excluding raw frames.
pub fn copy_conversation_024(
    policy: &IngressOwnershipPolicy,
    source: &ws024::Message,
) -> Result<AdmittedFrame<ws024::Message>> {
    view_024(source)?.conversation(policy)
}

/// Copy a decoded 0.29 message into the conversation port without sharing its
/// Bytes backing. The source and destination need simultaneous allowances.
pub fn copy_conversation_029(
    policy: &IngressOwnershipPolicy,
    source: &ws029::Message,
) -> Result<AdmittedFrame<ws024::Message>> {
    view_029(source)?.conversation(policy)
}
