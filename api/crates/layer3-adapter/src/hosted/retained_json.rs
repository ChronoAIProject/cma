//! Admit one parsed JSON tree and retain its reservation through RPC consumers.

use std::{
    borrow::Borrow,
    fmt,
    ops::Deref,
    sync::{Arc, OnceLock},
};

use crate::ingress_budget::{IngressBudget, IngressReservation};
use serde_json::Value;
use tokio::sync::Semaphore;

use super::output_json_budget;
use crate::AdapterError;

const TREE_BYTES: usize = 32 * 1024 * 1024;
const SCRATCH_BYTES: usize = 13 * 1024 * 1024;
const NODE_ALLOWANCE: usize = 1024;
const FIXED_ALLOWANCE: usize = 1024;

#[derive(Clone)]
pub(super) struct JsonPools {
    trees: Arc<Semaphore>,
    scratch: Arc<Semaphore>,
    aggregate: Option<IngressBudget>,
}

impl JsonPools {
    pub(super) fn process() -> Self {
        static POOLS: OnceLock<JsonPools> = OnceLock::new();
        POOLS
            .get_or_init(|| Self::new(TREE_BYTES, SCRATCH_BYTES))
            .clone()
    }

    pub(super) fn new(trees: usize, scratch: usize) -> Self {
        Self {
            trees: Arc::new(Semaphore::new(trees)),
            scratch: Arc::new(Semaphore::new(scratch)),
            aggregate: None,
        }
    }

    pub(super) fn with_aggregate(mut self, aggregate: Option<IngressBudget>) -> Self {
        self.aggregate = aggregate;
        self
    }

    pub(super) fn admit(&self, payload: &[u8]) -> crate::Result<JsonAdmission> {
        if payload.len() > 4 * 1024 * 1024 {
            return Err(output_json_budget::Refusal::Bytes.adapter_error());
        }
        // The borrowed preflight may allocate an unescape scratch Vec. Reserve
        // its complete allowance before running even that first parse.
        let scratch = acquire(
            &self.scratch,
            self.aggregate.as_ref(),
            scratch_allowance(payload.len())?,
        )?;
        let shape = output_json_budget::inspect(payload).map_err(|e| e.adapter_error())?;
        let tree = acquire(
            &self.trees,
            self.aggregate.as_ref(),
            tree_allowance(payload.len(), shape.nodes)?,
        )?;
        Ok(JsonAdmission {
            tree,
            _scratch: scratch,
        })
    }

    pub(super) fn decode(&self, payload: &[u8]) -> crate::Result<JsonFrame> {
        let admission = self.admit(payload)?;
        let value = serde_json::from_slice(payload)
            .map_err(|_| output_json_budget::Refusal::InvalidJson.adapter_error())?;
        Ok(JsonFrame {
            storage: Storage::Accounted(JsonOwner {
                value: Arc::new(value),
                _storage: Arc::new(admission.tree),
            }),
            result: false,
        })
    }
}

pub(super) struct JsonAdmission {
    tree: IngressReservation,
    _scratch: IngressReservation,
}

fn scratch_allowance(bytes: usize) -> crate::Result<usize> {
    bytes
        .checked_mul(3)
        .and_then(|v| v.checked_add(FIXED_ALLOWANCE))
        .ok_or_else(capacity)
}

fn string_allowance(bytes: usize) -> crate::Result<usize> {
    bytes
        .checked_mul(2)
        .and_then(|v| v.checked_add(FIXED_ALLOWANCE))
        .ok_or_else(capacity)
}

fn tree_allowance(bytes: usize, nodes: usize) -> crate::Result<usize> {
    string_allowance(bytes)?
        .checked_add(nodes.checked_mul(NODE_ALLOWANCE).ok_or_else(capacity)?)
        .ok_or_else(capacity)
}

fn acquire(
    pool: &Arc<Semaphore>,
    aggregate: Option<&IngressBudget>,
    bytes: usize,
) -> crate::Result<IngressReservation> {
    IngressReservation::try_acquire(pool, aggregate, bytes).ok_or_else(capacity)
}

fn capacity() -> AdapterError {
    AdapterError::AtCapacity("retained JSON storage capacity exhausted".into())
}

#[derive(Clone)]
struct JsonOwner {
    // Each clone holds both private owners. Field order deallocates the final
    // tree Arc (including its header) before releasing the last reservation.
    value: Arc<Value>,
    _storage: Arc<IngressReservation>,
}

#[derive(Clone)]
enum Storage {
    Legacy(Value),
    Accounted(JsonOwner),
}

/// The value is immutable and cannot be extracted separately from its owner.
#[derive(Clone)]
pub(super) struct JsonFrame {
    storage: Storage,
    result: bool,
}

impl JsonFrame {
    pub(super) fn legacy(value: Value) -> Self {
        Self {
            storage: Storage::Legacy(value),
            result: false,
        }
    }

    pub(super) fn into_result(self) -> Self {
        match self.storage {
            Storage::Legacy(value) => {
                Self::legacy(value.get("result").cloned().unwrap_or(Value::Null))
            }
            storage => Self {
                storage,
                result: true,
            },
        }
    }
}

impl Deref for JsonFrame {
    type Target = Value;

    fn deref(&self) -> &Value {
        let value = match &self.storage {
            Storage::Legacy(value) => value,
            Storage::Accounted(owner) => &owner.value,
        };
        if self.result {
            value.get("result").unwrap_or(&Value::Null)
        } else {
            value
        }
    }
}

impl Borrow<Value> for JsonFrame {
    fn borrow(&self) -> &Value {
        self
    }
}

impl fmt::Debug for JsonFrame {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.deref().fmt(formatter)
    }
}

#[cfg(test)]
impl JsonPools {
    pub(super) fn available(&self) -> (usize, usize) {
        (
            self.trees.available_permits(),
            self.scratch.available_permits(),
        )
    }
}

#[cfg(test)]
#[path = "retained_json_allocation_tests.rs"]
mod allocation_tests;
#[cfg(test)]
#[path = "retained_json_tests.rs"]
mod tests;
