//! Admit one parsed JSON tree and retain its reservation through RPC consumers.

use crate::ingress_budget::diagnostic::text;
use std::{
    borrow::Borrow,
    fmt,
    ops::Deref,
    sync::{Arc, OnceLock},
};

use crate::ingress_budget::{custody::SharedCustody, IngressBudget, IngressReservation};
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
    scratch_aggregate: Option<IngressBudget>,
    sign_ins: Arc<Semaphore>,
    observations: Arc<Semaphore>,
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
            scratch_aggregate: None,
            sign_ins: super::sign_in_retention::process_pool(),
            observations: super::observation_retention::process_pool(),
        }
    }

    pub(super) fn with_aggregate(mut self, aggregate: Option<IngressBudget>) -> Self {
        self.scratch_aggregate = aggregate.clone();
        self.aggregate = aggregate;
        self
    }

    pub(super) fn with_ownership(
        mut self,
        policy: &crate::ingress_budget::IngressOwnershipPolicy,
    ) -> Self {
        self.aggregate = Some(policy.budget(crate::ingress_budget::IngressClass::Data));
        self.scratch_aggregate =
            Some(policy.budget(crate::ingress_budget::IngressClass::Transformation));
        self
    }

    pub(super) fn admit_sign_in(
        &self,
        frame: &Value,
    ) -> crate::Result<Option<super::sign_in_retention::Admission>> {
        super::sign_in_retention::admit(frame, &self.sign_ins, self.aggregate.as_ref())
    }

    pub(super) fn admit_observation(
        &self,
        frame: &Value,
    ) -> crate::Result<Option<crate::RuntimeOutputUpdate>> {
        super::observation_retention::admit(frame, &self.observations, self.aggregate.as_ref())
    }

    pub(super) fn admit(&self, payload: &[u8]) -> crate::Result<JsonAdmission> {
        if payload.len() > 4 * 1024 * 1024 {
            return Err(output_json_budget::Refusal::Bytes.adapter_error());
        }
        // The borrowed preflight may allocate an unescape scratch Vec. Reserve
        // its complete allowance before running even that first parse.
        let scratch = acquire(
            &self.scratch,
            self.scratch_aggregate.as_ref(),
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
                _storage: SharedCustody::new(admission.tree),
            }),
            result: false,
            prepared_update: None,
            prepared_terminal: None,
            delivery_storage: None,
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
    AdapterError::AtCapacity(text("retained JSON storage capacity exhausted"))
}

#[derive(Clone)]
struct JsonOwner {
    // Each clone holds both private owners. Field order deallocates the final
    // tree Arc (including its header) before releasing the last reservation.
    value: Arc<Value>,
    _storage: SharedCustody<IngressReservation>,
}

#[derive(Clone)]
enum Storage {
    Legacy(Value),
    Accounted(JsonOwner),
    Admitted(crate::ingress_budget::SharedOwned<Value>),
}

/// The value is immutable and cannot be extracted separately from its owner.
#[derive(Clone)]
pub(super) struct JsonFrame {
    storage: Storage,
    result: bool,
    prepared_update: Option<super::acp::ingress::PreparedUpdate>,
    prepared_terminal: Option<crate::RuntimeOutputUpdate>,
    delivery_storage: Option<crate::ingress_budget::IngressWorkScope>,
}

impl JsonFrame {
    pub(super) fn prepare(
        params: &impl serde::Serialize,
        policy: Option<&crate::ingress_budget::IngressOwnershipPolicy>,
    ) -> crate::Result<Self> {
        let storage = super::acp::ingress::identity::json_storage(policy, params)?;
        let build = || {
            serde_json::to_value(params).map_err(|_| {
                AdapterError::Rejected(text("runtime parameters could not be prepared"))
            })
        };
        match storage {
            Some(storage) => Ok(Self::admitted(storage.try_initialize(build)?)),
            None => Ok(Self::legacy(build()?)),
        }
    }

    pub(super) fn admitted(value: crate::ingress_budget::SharedOwned<Value>) -> Self {
        Self {
            storage: Storage::Admitted(value),
            ..Self::legacy(Value::Null)
        }
    }

    pub(super) fn legacy(value: Value) -> Self {
        Self {
            storage: Storage::Legacy(value),
            result: false,
            prepared_update: None,
            prepared_terminal: None,
            delivery_storage: None,
        }
    }

    pub(super) fn with_prepared_update(
        mut self,
        update: Option<super::acp::ingress::PreparedUpdate>,
    ) -> Self {
        self.prepared_update = update;
        self
    }

    pub(super) fn prepared_update(&self) -> Option<super::acp::ingress::PreparedUpdate> {
        self.prepared_update.clone()
    }

    pub(super) fn with_prepared_terminal(
        mut self,
        terminal: Option<crate::RuntimeOutputUpdate>,
    ) -> Self {
        self.prepared_terminal = terminal;
        self
    }

    pub(super) fn prepared_terminal(&self) -> Option<crate::RuntimeOutputUpdate> {
        self.prepared_terminal.clone()
    }

    pub(super) fn with_delivery_storage(
        mut self,
        storage: crate::ingress_budget::IngressWorkScope,
    ) -> Self {
        self.delivery_storage = Some(storage);
        self
    }

    pub(super) fn delivery_storage(&self) -> Option<&crate::ingress_budget::IngressWorkScope> {
        self.delivery_storage.as_ref()
    }

    /// Share only admitted storage; legacy values must not be deep-cloned here.
    pub(super) fn shared(&self) -> Option<Self> {
        match &self.storage {
            Storage::Accounted(_) | Storage::Admitted(_) => Some(self.clone()),
            Storage::Legacy(_) => None,
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
                prepared_update: self.prepared_update,
                prepared_terminal: self.prepared_terminal,
                delivery_storage: self.delivery_storage,
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
            Storage::Admitted(owner) => owner,
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
    pub(super) fn with_observation_pool(mut self, pool: Arc<Semaphore>) -> Self {
        self.observations = pool;
        self
    }

    pub(super) fn with_sign_in_pool(mut self, pool: Arc<Semaphore>) -> Self {
        self.sign_ins = pool;
        self
    }

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
