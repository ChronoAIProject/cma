//! Bounded correlation storage; returned replies keep their channel allocation.

use super::super::PendingCall;
use crate::hosted::retained_json::JsonFrame;
use crate::ingress_budget::diagnostic::text;
use crate::ingress_budget::{
    AllocationPlan, IngressClass, IngressOwnershipPolicy, IngressWorkScope, ReservationBundle,
};
use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

const MAX_PENDING_CALLS: usize = 128;

pub(in crate::hosted) fn bridge_identity_storage(
    policy: Option<&IngressOwnershipPolicy>,
) -> crate::Result<Option<ReservationBundle>> {
    policy
        .map(|policy| {
            policy.reserve(
                IngressClass::Data,
                AllocationPlan::new(
                    "bridge correlation identities",
                    // Hex formatting can grow while the previous buffer remains live.
                    // The RPC namespace is a canonical UUID, independent of the digest.
                    &[3 * 64, 2 * 36],
                )?,
            )
        })
        .transpose()
}

pub(in crate::hosted) fn http_descriptor_plan(
    endpoint_bytes: usize,
    credential_bytes: Option<usize>,
) -> crate::Result<AllocationPlan> {
    use agent_client_protocol_schema::v1::HttpHeader;
    use serde_json::Value;
    let strings = AllocationPlan::new(
        "HTTP descriptor strings",
        &[
            endpoint_bytes,
            credential_bytes.unwrap_or(0),
            concat!(
                "type",
                "name",
                "url",
                "headers",
                "name",
                "value",
                "http",
                "cma_channel",
                "Authorization",
                "Bearer "
            )
            .len(),
        ],
    )?
    .requested_bytes()
    .checked_mul(4)
    .ok_or_else(|| crate::AdapterError::Rejected(text("HTTP descriptor layout overflow")))?;
    // Schema 1.11 builds an HTTP descriptor with four fields and at most one
    // two-field header. Cover typed strings (including format growth), the
    // serialized tree and its session-parameter copy. Each map entry may own a
    // whole BTree node. Array capacities include the minimum four-slot growth.
    let node = 11 * size_of::<(String, Value)>() + 14 * size_of::<usize>();
    AllocationPlan::new(
        "HTTP tool descriptor preparation",
        &[
            strings,
            2 * 6 * node,
            4 * size_of::<HttpHeader>(),
            8 * size_of::<Value>(),
        ],
    )
}

pub(in crate::hosted) fn handshake_plan(
    cwd: &str,
    settings: &crate::hosted::HostedAgentOpenSettings,
    correlation_bytes: usize,
) -> crate::Result<AllocationPlan> {
    let descriptors = [
        settings
            .channel_tool
            .as_ref()
            .map(|value| value.acp_server_plan()),
        settings
            .provider_tool
            .as_ref()
            .map(|value| value.acp_server_plan()),
        settings
            .access_tool
            .as_ref()
            .map(|value| value.acp_server_plan()),
    ];
    let mut tools = 0usize;
    for descriptor in descriptors.into_iter().flatten() {
        tools = tools
            .checked_add(descriptor?.requested_bytes())
            .ok_or_else(|| {
                crate::AdapterError::Rejected(text("handshake descriptor layout overflow"))
            })?;
    }
    let node = 11 * size_of::<(String, serde_json::Value)>() + 14 * size_of::<usize>();
    // Initialization has twelve map entries; load/new/mode needs at most three.
    // Their source JSON precedes begin()'s independently admitted request copy.
    // Include both tool-list vectors, initialize's capability vector, two local
    // session copies, and timing's parent/subphase/temporary-rebind identities.
    AllocationPlan::new(
        "handshake request preparation",
        &[
            tools,
            15 * node,
            12 * size_of::<serde_json::Value>(),
            concat!(
                "protocolVersionclientCapabilitiesfsreadTextFilewriteTextFile",
                "elicitationurl_metajetbrainsairversioncapabilitiessessionFailure",
                "sessionIdcwdmcpServersmodeId"
            )
            .len(),
            cwd.len(),
            settings.permission_mode.as_ref().map_or(0, String::len),
            2 * 1024,
            3 * 36,
            correlation_bytes,
            correlation_bytes,
        ],
    )
}

pub(in crate::hosted) fn initialization_params() -> serde_json::Value {
    serde_json::json!({
        "protocolVersion": super::super::PROTOCOL_VERSION,
        "clientCapabilities": {
            "fs": { "readTextFile": false, "writeTextFile": false },
            "elicitation": { "url": {} },
            // Negotiate native structured failures instead of assistant text
            // followed by end_turn; this is part of the existing wire contract.
            "_meta": {
                "jetbrains": { "air": { "version": 1, "capabilities": ["sessionFailure"] } }
            },
        },
    })
}

pub(in crate::hosted) fn json_storage(
    policy: Option<&IngressOwnershipPolicy>,
    value: &impl serde::Serialize,
) -> crate::Result<Option<ReservationBundle>> {
    policy
        .map(|policy| {
            policy.reserve(
                IngressClass::Data,
                AllocationPlan::new(
                    "outgoing JSON preparation",
                    &[
                        super::typed::Shape::serialized(value)?.retained::<serde_json::Value>()?,
                        // The sessionId/prompt wrapper and their copied keys/identity.
                        2 * (11 * size_of::<(String, serde_json::Value)>()
                            + 14 * size_of::<usize>()),
                        1024 + 32,
                    ],
                )?,
            )
        })
        .transpose()
}

pub(in crate::hosted) struct PendingCalls {
    calls: HashMap<String, PendingCall>,
    policy: Option<IngressOwnershipPolicy>,
    _storage: Option<ReservationBundle>,
}

pub(in crate::hosted) struct PendingReply {
    receiver: tokio::sync::oneshot::Receiver<JsonFrame>,
    _storage: Option<IngressWorkScope>,
}

impl Future for PendingReply {
    type Output = Result<JsonFrame, tokio::sync::oneshot::error::RecvError>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.receiver).poll(cx)
    }
}

impl PendingCalls {
    pub(in crate::hosted) fn new(policy: Option<&IngressOwnershipPolicy>) -> crate::Result<Self> {
        // hashbrown's 7/8 load factor rounds 128 entries to 256 buckets.
        // Include each value, its control byte, a SIMD group and alignment.
        let buckets = (MAX_PENDING_CALLS * 8).div_ceil(7).next_power_of_two();
        let storage = policy
            .map(|policy| {
                policy.reserve(
                    IngressClass::Data,
                    AllocationPlan::new(
                        "pending RPC table",
                        &[
                            buckets * (size_of::<(String, PendingCall)>() + 1),
                            16 + align_of::<(String, PendingCall)>(),
                        ],
                    )?,
                )
            })
            .transpose()?;
        Ok(Self {
            calls: match policy {
                Some(_) => HashMap::with_capacity(MAX_PENDING_CALLS),
                None => HashMap::new(),
            },
            policy: policy.cloned(),
            _storage: storage,
        })
    }

    pub(in crate::hosted) fn register(
        &mut self,
        id: &str,
        prompt: bool,
    ) -> crate::Result<PendingReply> {
        if self.policy.is_some()
            && (self.calls.len() == MAX_PENDING_CALLS || self.calls.contains_key(id))
        {
            return Err(crate::AdapterError::AtCapacity(text(
                "pending runtime calls reached their bound",
            )));
        }
        // Tokio 1.53 oneshot Inner has an atomic state, value slot and two
        // waker cells. Each cell holds two pointers plus a flag; Arc adds two
        // counters. Alignment is rounded conservatively by another two words.
        let storage = self
            .policy
            .as_ref()
            .map(|policy| {
                policy
                    .reserve(
                        IngressClass::Data,
                        AllocationPlan::new(
                            "pending RPC correlation",
                            &[
                                id.len(),
                                size_of::<Option<JsonFrame>>() + 12 * size_of::<usize>(),
                                crate::ingress_budget::sizing::shared_owner_bytes::<()>(),
                            ],
                        )?,
                    )
                    .map(IngressWorkScope::new)
            })
            .transpose()?;
        let (answer, receiver) = tokio::sync::oneshot::channel();
        self.calls.insert(
            id.to_owned(),
            PendingCall {
                answer,
                prompt,
                _storage: storage.clone(),
            },
        );
        Ok(PendingReply {
            receiver,
            _storage: storage,
        })
    }

    pub(in crate::hosted) fn get(&self, id: &str) -> Option<&PendingCall> {
        self.calls.get(id)
    }
    pub(in crate::hosted) fn remove(&mut self, id: &str) -> Option<PendingCall> {
        self.calls.remove(id)
    }
    pub(in crate::hosted) fn clear(&mut self) {
        self.calls.clear();
    }
    pub(in crate::hosted) fn values(&self) -> impl Iterator<Item = &PendingCall> {
        self.calls.values()
    }
    #[cfg(test)]
    pub(in crate::hosted) fn is_empty(&self) -> bool {
        self.calls.is_empty()
    }
}

pub(in crate::hosted) fn request_scope(
    policy: &IngressOwnershipPolicy,
    params: &serde_json::Value,
    method: &str,
    namespace: &str,
) -> crate::Result<IngressWorkScope> {
    let shape = super::typed::Shape::inspect(params)?;
    // The generated id is a namespace, an i64 decimal and separators; durable
    // prompt ids are 77 bytes. json! copies both params and these envelope fields.
    let id = namespace
        .len()
        .checked_add(32)
        .map(|n| n.max(77))
        .ok_or_else(|| crate::AdapterError::Rejected(text("RPC correlation layout overflow")))?;
    let node = 11 * size_of::<(String, serde_json::Value)>() + 14 * size_of::<usize>();
    policy
        .reserve(
            IngressClass::Data,
            AllocationPlan::new(
                "RPC request preparation",
                &[
                    shape.retained::<serde_json::Value>()?,
                    4 * node,
                    3 * id,
                    method.len(),
                    64,
                ],
            )?,
        )
        .map(IngressWorkScope::new)
}
