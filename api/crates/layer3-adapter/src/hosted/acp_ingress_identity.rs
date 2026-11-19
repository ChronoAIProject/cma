//! Bounded correlation storage; returned replies keep their channel allocation.

use super::super::PendingCall;
use crate::hosted::retained_json::JsonFrame;
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
            return Err(crate::AdapterError::AtCapacity(
                "pending runtime calls reached their bound".into(),
            ));
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
        .ok_or_else(|| crate::AdapterError::Rejected("RPC correlation layout overflow".into()))?;
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
