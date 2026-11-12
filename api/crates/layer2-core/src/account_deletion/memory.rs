//! In-memory implementation for deterministic domain and interface fixtures.
use super::*;
use crate::SubjectId;
use async_trait::async_trait;
use std::{collections::BTreeMap, sync::Mutex, time::Duration};

#[derive(Default)]
pub struct MemoryAccountDeletionStore(Mutex<State>);
#[derive(Default)]
struct State {
    records: BTreeMap<AccountDeletionId, AccountDeletion>,
    keys: BTreeMap<(String, String), AccountDeletionId>,
    events: BTreeMap<AccountDeletionId, Vec<ErasureEvent>>,
}
#[async_trait]
impl AccountDeletionStore for MemoryAccountDeletionStore {
    async fn replay(
        &self,
        subject: &SubjectId,
        key: &str,
    ) -> Result<Option<AccountDeletion>, AccountDeletionError> {
        let state = self
            .0
            .lock()
            .map_err(|_| AccountDeletionError::Store("memory authority poisoned".into()))?;
        let identity = (
            subject_hash(subject),
            crate::document_artifacts::digest(key.as_bytes()),
        );
        Ok(state
            .keys
            .get(&identity)
            .and_then(|id| state.records.get(id))
            .cloned())
    }
    async fn request(
        &self,
        subject: &SubjectId,
        key: &str,
        at: &str,
    ) -> Result<AccountDeletion, AccountDeletionError> {
        let mut state = self
            .0
            .lock()
            .map_err(|_| AccountDeletionError::Store("memory authority poisoned".into()))?;
        let hash = subject_hash(subject);
        let identity = (
            hash.clone(),
            crate::document_artifacts::digest(key.as_bytes()),
        );
        if let Some(record) = state
            .keys
            .get(&identity)
            .and_then(|id| state.records.get(id))
        {
            return Ok(record.clone());
        }
        if let Some(record) = state
            .records
            .values()
            .find(|record| record.subject_hash == hash && record.is_active())
            .cloned()
        {
            return Ok(record);
        }
        let record = AccountDeletion::request(AccountDeletionId::generate(), subject.clone(), at)?;
        state.keys.insert(identity, record.id.clone());
        state.events.insert(
            record.id.clone(),
            vec![record.current_event(at, ErasureEventKind::Started)],
        );
        state.records.insert(record.id.clone(), record.clone());
        Ok(record)
    }
    async fn active(
        &self,
        subject: &SubjectId,
    ) -> Result<Option<AccountDeletion>, AccountDeletionError> {
        Ok(self
            .0
            .lock()
            .map_err(|_| AccountDeletionError::Store("memory authority poisoned".into()))?
            .records
            .values()
            .find(|record| record.subject_hash == subject_hash(subject) && record.is_active())
            .cloned())
    }
    async fn latest(
        &self,
        subject: &SubjectId,
    ) -> Result<Option<AccountDeletion>, AccountDeletionError> {
        Ok(self
            .0
            .lock()
            .map_err(|_| AccountDeletionError::Store("memory authority poisoned".into()))?
            .records
            .values()
            .filter(|record| record.subject_hash == subject_hash(subject))
            .max_by_key(|record| {
                (
                    record.is_active(),
                    record.requested_at.clone(),
                    record.id.clone(),
                )
            })
            .cloned())
    }
    async fn get(
        &self,
        id: &AccountDeletionId,
    ) -> Result<Option<AccountDeletion>, AccountDeletionError> {
        Ok(self
            .0
            .lock()
            .map_err(|_| AccountDeletionError::Store("memory authority poisoned".into()))?
            .records
            .get(id)
            .cloned())
    }
    async fn claim_due(
        &self,
        holder: &str,
        now: &str,
        lease: Duration,
    ) -> Result<Option<AccountDeletion>, AccountDeletionError> {
        let mut state = self
            .0
            .lock()
            .map_err(|_| AccountDeletionError::Store("memory authority poisoned".into()))?;
        let Some(record) = state.records.values_mut().find(|record| record.is_due(now)) else {
            return Ok(None);
        };
        record.claim(holder, now, lease)?;
        let record = record.clone();
        state
            .events
            .entry(record.id.clone())
            .or_default()
            .push(record.current_event(now, ErasureEventKind::Started));
        Ok(Some(record))
    }
    async fn commit(
        &self,
        deletion: AccountDeletion,
        events: Vec<ErasureEvent>,
        expected_revision: u64,
        expected_lease_token: Option<&str>,
    ) -> Result<(), AccountDeletionError> {
        let mut state = self
            .0
            .lock()
            .map_err(|_| AccountDeletionError::Store("memory authority poisoned".into()))?;
        let old = state
            .records
            .get(&deletion.id)
            .ok_or(AccountDeletionError::NotFound)?;
        if old.revision != expected_revision
            || deletion.revision <= old.revision
            || deletion.work.next_fence != old.work.next_fence
        {
            return Err(AccountDeletionError::Conflict);
        }
        if old.work.lease.as_ref().map(|lease| lease.token.as_str()) != expected_lease_token {
            return Err(AccountDeletionError::StaleFence);
        }
        if deletion.work.lease.as_ref().is_some_and(|lease| {
            Some(lease.token.as_str()) != expected_lease_token
                || old
                    .work
                    .lease
                    .as_ref()
                    .is_none_or(|old| old.fence != lease.fence)
        }) {
            return Err(AccountDeletionError::StaleFence);
        }
        if old.event_seq.checked_add(events.len() as u64) != Some(deletion.event_seq)
            || events.iter().enumerate().any(|(index, event)| {
                event.deletion_id != deletion.id || event.seq != old.event_seq + index as u64 + 1
            })
        {
            return Err(AccountDeletionError::Invalid);
        }
        state
            .events
            .entry(deletion.id.clone())
            .or_default()
            .extend(events);
        state.records.insert(deletion.id.clone(), deletion);
        Ok(())
    }
}

#[cfg(test)]
#[path = "memory_tests.rs"]
mod tests;
