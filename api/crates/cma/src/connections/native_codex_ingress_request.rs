//! Fixed request bytes for the future qualified native refresh worker.

use std::io::{self, Write as _};

use layer3_adapter::ingress_budget::{
    sizing, AdmissionRefusal, AllocationPlan, IngressClass, IngressOwnershipPolicy,
    IngressWorkScope, ReservationBundle, SharedOwned,
};
use serde::Serialize;
use zeroize::Zeroizing;

use super::{ingress_protocol::NativeFrameError, SecretValue};

const INITIALIZE_FIXED: usize = br#"{"id":,"method":"initialize","params":{"clientInfo":{"name":"cma_subscription_connection","version":""},"capabilities":{"experimentalApi":false}}}"#.len();
const READ_ACCOUNT_FIXED: usize =
    br#"{"id":,"method":"account/read","params":{"refreshToken":true}}"#.len();
const INITIALIZED_BYTES: usize = br#"{"method":"initialized"}"#.len();
const U64_DIGITS: usize = 20;

// serde_json 1.0.154 ErrorImpl contains ErrorCode and two usize positions.
// Its largest code payload is Box<str> (two words), with a tag; Io(Error)
// needs one word. Eight words cover the boxed layout, not just Error's pointer.
// Only Error::io with our payload-free ErrorKind is reachable while encoding.
const SERIALIZER_ERROR_BYTES: usize = size_of::<[usize; 8]>();

#[derive(Clone, Copy, Debug)]
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "native refresh request qualification pending")
)]
pub(super) enum NativeRefreshRequest {
    Initialize { id: u64 },
    Initialized,
    ReadAccount { id: u64 },
}

#[cfg_attr(
    not(test),
    expect(dead_code, reason = "native refresh request qualification pending")
)]
impl NativeRefreshRequest {
    pub(super) fn encode_owned(
        self,
        source: &IngressWorkScope,
        policy: &IngressOwnershipPolicy,
    ) -> Result<SharedOwned<SecretValue>, NativeFrameError> {
        let version = env!("CARGO_PKG_VERSION");
        let capacity = self.maximum_bytes(version)?;
        RequestBuffer::admit(source, policy, capacity)?.encode(self, version)
    }

    fn maximum_bytes(self, version: &str) -> Result<usize, AdmissionRefusal> {
        match self {
            Self::Initialize { .. } => {
                checked_sum(&[INITIALIZE_FIXED, U64_DIGITS, escaped_bytes(version)?, 1])
            }
            Self::Initialized => checked_sum(&[INITIALIZED_BYTES, 1]),
            Self::ReadAccount { .. } => checked_sum(&[READ_ACCOUNT_FIXED, U64_DIGITS, 1]),
        }
    }

    fn write(self, writer: &mut BoundedWriter<'_>, version: &str) -> serde_json::Result<()> {
        #[cfg(test)]
        tests::SERIALIZER_ENTRIES.with(|entries| entries.set(entries.get() + 1));
        match self {
            Self::Initialize { id } => serde_json::to_writer(
                writer,
                &Request {
                    id,
                    method: "initialize",
                    params: Initialize {
                        client_info: ClientInfo {
                            name: "cma_subscription_connection",
                            version,
                        },
                        capabilities: Capabilities {
                            experimental_api: false,
                        },
                    },
                },
            ),
            Self::Initialized => serde_json::to_writer(
                writer,
                &Notification {
                    method: "initialized",
                },
            ),
            Self::ReadAccount { id } => serde_json::to_writer(
                writer,
                &Request {
                    id,
                    method: "account/read",
                    params: ReadAccount {
                        refresh_token: true,
                    },
                },
            ),
        }
    }
}

#[derive(Serialize)]
struct Request<P> {
    id: u64,
    method: &'static str,
    params: P,
}

#[derive(Serialize)]
struct Notification {
    method: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Initialize<'a> {
    client_info: ClientInfo<'a>,
    capabilities: Capabilities,
}

#[derive(Serialize)]
struct ClientInfo<'a> {
    name: &'static str,
    version: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Capabilities {
    experimental_api: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReadAccount {
    refresh_token: bool,
}

struct RequestBuffer {
    bytes: Zeroizing<Vec<u8>>,
    storage: ReservationBundle,
}

impl RequestBuffer {
    fn admit(
        source: &IngressWorkScope,
        policy: &IngressOwnershipPolicy,
        capacity: usize,
    ) -> Result<Self, NativeFrameError> {
        if !source.shares_custody_domain(policy) {
            return Err(AdmissionRefusal::InvalidPlan {
                phase: "native refresh request ownership",
            }
            .into());
        }
        let storage = policy.admit(IngressClass::Data, request_plan(capacity)?)?;
        Ok(Self {
            bytes: Zeroizing::new(vec![0; capacity]),
            storage,
        })
    }

    fn encode(
        self,
        request: NativeRefreshRequest,
        version: &str,
    ) -> Result<SharedOwned<SecretValue>, NativeFrameError> {
        self.finish(|writer| {
            request
                .write(writer, version)
                .map_err(|_| NativeFrameError::Unavailable)?;
            writer
                .write_all(b"\n")
                .map_err(|_| NativeFrameError::Unavailable)
        })
    }

    fn finish(
        mut self,
        write: impl FnOnce(&mut BoundedWriter<'_>) -> Result<(), NativeFrameError>,
    ) -> Result<SharedOwned<SecretValue>, NativeFrameError> {
        let capacity = self.bytes.capacity();
        let used = {
            let mut writer = BoundedWriter {
                bytes: &mut self.bytes,
                used: 0,
            };
            write(&mut writer)?;
            writer.used
        };
        self.bytes.truncate(used);
        let text = match String::from_utf8(std::mem::take(&mut *self.bytes)) {
            Ok(text) => text,
            Err(error) => {
                drop(Zeroizing::new(error.into_bytes()));
                return Err(NativeFrameError::Unavailable);
            }
        };
        let value = SecretValue::new(text);
        let result = self
            .storage
            .split_admitted(output_plan(capacity)?)?
            .initialize(|| value);
        // Any serializer error was dropped before finish returned or reached
        // this point; the diagnostic allowance is released after that lifetime.
        Ok(result)
    }
}

struct BoundedWriter<'a> {
    bytes: &'a mut [u8],
    used: usize,
}

impl io::Write for BoundedWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.bytes.len() - self.used {
            return Err(io::ErrorKind::WriteZero.into());
        }
        self.bytes[self.used..self.used + bytes.len()].copy_from_slice(bytes);
        self.used += bytes.len();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn checked_sum(parts: &[usize]) -> Result<usize, AdmissionRefusal> {
    parts.iter().try_fold(0usize, |sum, part| {
        sum.checked_add(*part).ok_or(AdmissionRefusal::InvalidPlan {
            phase: "native refresh request layout",
        })
    })
}

fn escaped_bytes(value: &str) -> Result<usize, AdmissionRefusal> {
    value.bytes().try_fold(0, |count, byte| {
        let bytes = match byte {
            b'"' | b'\\' | b'\x08' | b'\t' | b'\n' | b'\x0c' | b'\r' => 2,
            0..=0x1f => 6,
            _ => 1,
        };
        checked_sum(&[count, bytes])
    })
}

fn output_plan(capacity: usize) -> Result<AllocationPlan, AdmissionRefusal> {
    AllocationPlan::checked(
        "native refresh request output",
        &[capacity, sizing::shared_owner_bytes::<SecretValue>()],
    )
}

fn request_plan(capacity: usize) -> Result<AllocationPlan, AdmissionRefusal> {
    AllocationPlan::checked(
        "native refresh request encoding",
        &[
            output_plan(capacity)?.requested_bytes(),
            SERIALIZER_ERROR_BYTES,
        ],
    )
}

#[cfg(test)]
#[path = "native_codex_ingress_request_tests.rs"]
mod tests;
