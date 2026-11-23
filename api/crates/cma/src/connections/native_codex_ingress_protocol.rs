//! Admitted frame bytes for the future qualified native protocol worker.

use layer3_adapter::ingress_budget::{
    sizing, AdmissionRefusal, AllocationPlan, IngressClass, IngressOwnershipPolicy,
    IngressWorkScope, ReservationBundle, SharedOwned,
};
use tokio::io::{AsyncBufRead, AsyncBufReadExt as _};
use zeroize::Zeroizing;

use super::SecretValue;

const MAX_FRAME_BYTES: usize = super::MAX_FRAME_BYTES as usize;
const READ_BYTES: usize = MAX_FRAME_BYTES + 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NativeFrameError {
    Refused(AdmissionRefusal),
    Unavailable,
}

impl From<AdmissionRefusal> for NativeFrameError {
    fn from(error: AdmissionRefusal) -> Self {
        Self::Refused(error)
    }
}

/// Owns frame bytes only. The caller must separately qualify the original
/// buffered stream, its backing and task. Domain membership is not stream identity.
pub(super) struct NativeFrameRead {
    bytes: Zeroizing<Vec<u8>>,
    storage: ReservationBundle,
}

/// Retains framing disposition when a read future is dropped. The caller must
/// qualify this inline state and its owned buffered transport separately.
pub(super) struct NativeFrameStream<R> {
    reader: R,
    usable: bool,
}

#[cfg_attr(
    not(test),
    expect(dead_code, reason = "native protocol stream qualification pending")
)]
impl<R: AsyncBufRead + Unpin + Send + 'static> NativeFrameStream<R> {
    pub(super) fn new(reader: R) -> Self {
        Self {
            reader,
            usable: true,
        }
    }

    pub(super) fn is_usable(&self) -> bool {
        self.usable
    }
}

#[cfg_attr(
    not(test),
    expect(dead_code, reason = "native protocol worker qualification pending")
)]
impl NativeFrameRead {
    pub(super) fn admit(
        source: &IngressWorkScope,
        policy: &IngressOwnershipPolicy,
    ) -> Result<Self, NativeFrameError> {
        if !source.shares_custody_domain(policy) {
            return Err(AdmissionRefusal::InvalidPlan {
                phase: "native frame byte ownership",
            }
            .into());
        }
        let storage = policy.admit(IngressClass::Data, frame_plan()?)?;
        Ok(Self {
            bytes: Zeroizing::new(vec![0; READ_BYTES]),
            storage,
        })
    }

    /// Consume through the first newline, leaving later buffered frames intact.
    /// JSON parsing and its independently admitted Value storage happen later.
    pub(super) async fn read<R: AsyncBufRead + Unpin + Send + 'static>(
        mut self,
        stream: &mut NativeFrameStream<R>,
    ) -> Result<SharedOwned<SecretValue>, NativeFrameError> {
        if !stream.is_usable() {
            return Err(NativeFrameError::Unavailable);
        }
        let mut used = 0;
        while used < READ_BYTES {
            let available = match stream.reader.fill_buf().await {
                Ok(available) => available,
                Err(_) => {
                    stream.usable = false;
                    return Err(NativeFrameError::Unavailable);
                }
            };
            let limit = available.len().min(READ_BYTES - used);
            if limit == 0 {
                break;
            }
            let newline = available[..limit].iter().position(|byte| *byte == b'\n');
            let count = newline.map_or(limit, |index| index + 1);
            self.bytes[used..used + count].copy_from_slice(&available[..count]);
            // Set before consumption so cancellation/unwind cannot erase the
            // fact that a prefix may be gone. Only a complete owned frame clears it.
            stream.usable = false;
            stream.reader.consume(count);
            used += count;
            if newline.is_some() {
                break;
            }
        }
        if used == 0 || used > MAX_FRAME_BYTES || self.bytes[used - 1] != b'\n' {
            stream.usable = false;
            return Err(NativeFrameError::Unavailable);
        }
        // Preserve the admitted capacity. The unread suffix was zeroed before
        // the first read, so truncation cannot leave input outside the string.
        self.bytes.truncate(used);
        let bytes = std::mem::take(&mut *self.bytes);
        let text = match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(error) => {
                drop(Zeroizing::new(error.into_bytes()));
                return Err(NativeFrameError::Unavailable);
            }
        };
        let frame = self.storage.initialize(|| SecretValue::new(text));
        stream.usable = true;
        Ok(frame)
    }
}

fn frame_plan() -> Result<AllocationPlan, AdmissionRefusal> {
    AllocationPlan::checked(
        "native original frame bytes",
        &[READ_BYTES, sizing::shared_owner_bytes::<SecretValue>()],
    )
}

#[cfg(test)]
#[path = "native_codex_ingress_protocol_tests.rs"]
mod tests;
