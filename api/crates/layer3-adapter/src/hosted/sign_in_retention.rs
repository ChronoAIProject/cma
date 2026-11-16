//! Reserve selected sign-in strings before copying them out of an admitted frame.

use std::sync::{Arc, OnceLock};

use serde_json::Value;
use tokio::sync::Semaphore;

use crate::ingress_budget::{IngressBudget, IngressReservation};
use crate::{AdapterError, ConversationUpdate, RuntimeOutputUpdate};

const PROCESS_BYTES: usize = 8 * 1024 * 1024;
// Separate Arcs hold the effect and reservation, each with two counters. Count
// their explicit layouts too; allocator metadata and OS overhead remain separate.
const ENVELOPE_BYTES: usize = RuntimeOutputUpdate::ACCOUNTED_STORAGE_BYTES;

pub(super) struct Fields<'a> {
    pub(super) url: &'a str,
    pub(super) code: Option<&'a str>,
    pub(super) message: Option<&'a str>,
}

impl<'a> Fields<'a> {
    pub(super) fn read(params: Option<&'a Value>) -> Option<Self> {
        let text = |key: &str| params?.get(key)?.as_str();
        Some(Self {
            url: text("url")?,
            code: text("userCode").or_else(|| text("code")),
            message: text("message"),
        })
    }

    fn weight(&self) -> crate::Result<usize> {
        [Some(self.url), self.code, self.message]
            .into_iter()
            .flatten()
            .try_fold(ENVELOPE_BYTES, |total, value| {
                total.checked_add(value.len()).ok_or_else(capacity)
            })
    }

    pub(super) fn owned(&self) -> ConversationUpdate {
        ConversationUpdate::SignInRequired {
            url: self.url.to_owned(),
            code: self.code.map(str::to_owned),
            message: self.message.map(str::to_owned),
        }
    }
}

pub(super) struct Admission(IngressReservation);

impl Admission {
    pub(super) fn wrap(self, update: ConversationUpdate) -> ConversationUpdate {
        ConversationUpdate::RetainedEffect(RuntimeOutputUpdate::accounted(update, Arc::new(self.0)))
    }
}

pub(super) fn process_pool() -> Arc<Semaphore> {
    static POOL: OnceLock<Arc<Semaphore>> = OnceLock::new();
    POOL.get_or_init(|| Arc::new(Semaphore::new(PROCESS_BYTES)))
        .clone()
}

pub(super) fn admit(
    frame: &Value,
    pool: &Arc<Semaphore>,
    aggregate: Option<&IngressBudget>,
) -> crate::Result<Option<Admission>> {
    if frame.get("method").and_then(Value::as_str) != Some("elicitation/create") {
        return Ok(None);
    }
    let Some(fields) = Fields::read(frame.get("params")) else {
        return Ok(None);
    };
    let storage =
        IngressReservation::try_acquire(pool, aggregate, fields.weight()?).ok_or_else(capacity)?;
    Ok(Some(Admission(storage)))
}

fn capacity() -> AdapterError {
    AdapterError::AtCapacity("runtime sign-in notice storage capacity exhausted".into())
}

#[cfg(test)]
#[path = "sign_in_retention_tests.rs"]
mod tests;
