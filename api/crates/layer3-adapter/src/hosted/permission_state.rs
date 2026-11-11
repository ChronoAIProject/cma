//! Compact permission retention. Reservations cover requested storage, not RSS.

use std::sync::{
    atomic::{AtomicU8, Ordering},
    Arc, OnceLock,
};

use sha2::{Digest as _, Sha256};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::{AdapterError, PermissionDecision, PermissionOption, PermissionOptionKind, Result};

pub(super) const MAX_ENTRIES: usize = 128;
pub(super) const MAX_OPTIONS: usize = 64;
pub(super) const MAX_OPTION_BYTES: usize = 256;
const PROCESS_BYTES: usize = 8 * 1024 * 1024;
// Conservative padding for Arc/control/alignment, separate from allocator RSS.
const LAYOUT_ALLOWANCE: usize = 128;

pub(super) struct PermissionState {
    entries: Vec<Arc<Entry>>,
    budget: Arc<Semaphore>,
    limit: usize,
    scope: [u8; 32],
    active_prompt: bool,
    _table: OwnedSemaphorePermit,
}

struct Choice {
    id: Box<str>,
    kind: PermissionOptionKind,
}

pub(super) struct Entry {
    scope: [u8; 32],
    id: Box<str>,
    call: serde_json::Value,
    choices: Box<[Choice]>,
    selected: AtomicU8,
    pub(super) owns_prompt_waiter: bool,
    _storage: OwnedSemaphorePermit,
}

impl PermissionState {
    pub(super) fn new() -> Result<Self> {
        static BUDGET: OnceLock<Arc<Semaphore>> = OnceLock::new();
        Self::with_budget(
            BUDGET
                .get_or_init(|| Arc::new(Semaphore::new(PROCESS_BYTES)))
                .clone(),
            MAX_ENTRIES,
        )
    }

    fn with_budget(budget: Arc<Semaphore>, limit: usize) -> Result<Self> {
        let bytes = size_of::<Self>() + LAYOUT_ALLOWANCE + limit * size_of::<Arc<Entry>>();
        let table = reserve(&budget, bytes)?;
        Ok(Self {
            entries: Vec::with_capacity(limit),
            budget,
            limit,
            scope: [0; 32],
            active_prompt: false,
            _table: table,
        })
    }

    pub(super) fn get(&self, id: &str) -> Option<Arc<Entry>> {
        self.entries
            .iter()
            .find(|entry| entry.scope == self.scope && &*entry.id == id)
            .cloned()
    }

    pub(super) fn begin_prompt(&mut self, id: &str) {
        self.scope = prompt_scope(id, true);
        self.active_prompt = true;
    }

    pub(super) fn complete_prompt(&mut self, id: &str) {
        if self.active_prompt && self.scope != prompt_scope(id, true) {
            return;
        }
        // Keep old records owned, but never apply a prior turn's decision to a
        // reused RPC id. A foreign terminal cannot change an active scope.
        self.scope = prompt_scope(id, false);
        self.active_prompt = false;
    }

    pub(super) fn remember(
        &mut self,
        id: &str,
        call: &serde_json::Value,
        options: &[PermissionOption],
        owns_prompt_waiter: bool,
    ) -> Result<Arc<Entry>> {
        if let Some(entry) = self.get(id) {
            return Ok(entry);
        }
        validate(options)?;
        let call_bytes = match call {
            serde_json::Value::String(value) if value.len() <= 256 => value.len(),
            serde_json::Value::Number(_) => 256,
            _ => return Err(invalid()),
        };
        if id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(invalid());
        }
        if self.entries.len() == self.limit {
            return Err(capacity());
        }
        let bytes = size_of::<Entry>()
            + LAYOUT_ALLOWANCE
            + id.len()
            + call_bytes
            + options.len() * size_of::<Choice>()
            + options
                .iter()
                .map(|option| option.option_id.0.len())
                .sum::<usize>();
        // Admission precedes all copies retained by the table or an in-flight replay.
        let storage = reserve(&self.budget, bytes)?;
        let mut choices = Vec::with_capacity(options.len());
        for option in options {
            choices.push(Choice {
                id: option.option_id.0.as_ref().into(),
                kind: option.kind,
            });
        }
        let entry = Arc::new(Entry {
            scope: self.scope,
            id: id.into(),
            call: call.clone(),
            choices: choices.into_boxed_slice(),
            selected: AtomicU8::new(0),
            owns_prompt_waiter,
            _storage: storage,
        });
        self.entries.push(entry.clone());
        Ok(entry)
    }
}

fn prompt_scope(id: &str, active: bool) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"cma-permission-scope-v1\0");
    hash.update([u8::from(active)]);
    hash.update(id.as_bytes());
    hash.finalize().into()
}

impl Entry {
    pub(super) fn select(&self, decision: PermissionDecision) {
        let value = match decision {
            PermissionDecision::Allow => 1,
            PermissionDecision::Deny => 2,
            PermissionDecision::Abort => 3,
        };
        // The first durable intent wins even if its write is cancelled or uncertain.
        let _ = self
            .selected
            .compare_exchange(0, value, Ordering::SeqCst, Ordering::SeqCst);
    }

    pub(super) fn answer(&self) -> Option<serde_json::Value> {
        let wanted: &[PermissionOptionKind] = match self.selected.load(Ordering::SeqCst) {
            0 => return None,
            1 => &[
                PermissionOptionKind::AllowOnce,
                PermissionOptionKind::AllowAlways,
            ],
            2 => &[
                PermissionOptionKind::RejectOnce,
                PermissionOptionKind::RejectAlways,
            ],
            _ => &[],
        };
        let choice = wanted
            .iter()
            .find_map(|kind| self.choices.iter().find(|choice| &choice.kind == kind));
        let outcome = match choice {
            Some(choice) => serde_json::json!({"outcome":"selected", "optionId": &*choice.id}),
            None => serde_json::json!({"outcome":"cancelled"}),
        };
        Some(serde_json::json!({"jsonrpc":"2.0", "id":self.call, "result":{"outcome":outcome}}))
    }
}

pub(super) fn validate(options: &[PermissionOption]) -> Result<()> {
    if options.len() > MAX_OPTIONS
        || options
            .iter()
            .any(|option| option.option_id.0.len() > MAX_OPTION_BYTES)
    {
        return Err(invalid());
    }
    Ok(())
}

fn reserve(budget: &Arc<Semaphore>, bytes: usize) -> Result<OwnedSemaphorePermit> {
    let bytes = u32::try_from(bytes).map_err(|_| capacity())?;
    budget
        .clone()
        .try_acquire_many_owned(bytes)
        .map_err(|_| capacity())
}

fn capacity() -> AdapterError {
    AdapterError::Unavailable("runtime permission retention capacity exhausted".into())
}

fn invalid() -> AdapterError {
    AdapterError::Rejected("runtime permission retention shape exceeds its bounds".into())
}

#[cfg(test)]
#[path = "permission_state_tests.rs"]
mod tests;
