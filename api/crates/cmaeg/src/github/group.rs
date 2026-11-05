use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::model::TaskResult;

pub(super) const COALESCE_WINDOW_SECONDS: i64 = 30;
const MAX_BATCH_MEMBERS: usize = 64;
const MAX_MEMBER_BYTES: usize = 512 * 1024;
const MAX_GROUP_BYTES: usize = 4 * 1024 * 1024;
const MAX_SEEN_RECEIPTS: usize = 256;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Member {
    pub receipt_id: String,
    pub delivery_id: String,
    pub event: String,
    pub action: Option<String>,
    pub installation_id: u64,
    pub authority_revision: u64,
    pub repository_id: u64,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Batch {
    pub batch_id: String,
    pub generation: u64,
    pub started_at: i64,
    pub admission_id: Option<String>,
    pub launch_lease_until: i64,
    pub members: Vec<Member>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ExecutionGroup {
    pub group_id: String,
    pub owner: String,
    pub app_ref: String,
    pub channel_id: String,
    pub record_id: String,
    pub generation: u64,
    pub pending: Vec<Member>,
    pub active: Option<Batch>,
    pub next_flush_at: Option<i64>,
    pub seen_receipt_ids: Vec<String>,
    pub last_status: String,
    pub last_batch_id: Option<String>,
    pub last_admission_id: Option<String>,
    pub last_response_id: Option<String>,
    #[serde(default)]
    pub result: Option<TaskResult>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EnqueueResult {
    Added { flush_at: i64 },
    Duplicate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GroupError {
    Invalid,
    TooLarge,
    Full,
    WrongBatch,
}

impl ExecutionGroup {
    pub(super) fn new(
        group_id: String,
        owner: String,
        app_ref: String,
        channel_id: String,
        record_id: String,
        now: i64,
    ) -> Self {
        Self {
            group_id,
            owner,
            app_ref,
            channel_id,
            record_id,
            generation: 0,
            pending: Vec::new(),
            active: None,
            next_flush_at: None,
            seen_receipt_ids: Vec::new(),
            last_status: "queued".into(),
            last_batch_id: None,
            last_admission_id: None,
            last_response_id: None,
            result: None,
            updated_at: now,
        }
    }

    pub(super) fn enqueue(
        &mut self,
        member: Member,
        now: i64,
    ) -> Result<EnqueueResult, GroupError> {
        if member.receipt_id.is_empty()
            || member.delivery_id.is_empty()
            || member.event.is_empty()
            || member.repository_id == 0
            || !member.payload.is_object()
        {
            return Err(GroupError::Invalid);
        }
        if self
            .seen_receipt_ids
            .iter()
            .any(|id| id == &member.receipt_id)
        {
            return Ok(EnqueueResult::Duplicate);
        }
        if serde_json::to_vec(&member)
            .map_err(|_| GroupError::Invalid)?
            .len()
            > MAX_MEMBER_BYTES
        {
            return Err(GroupError::TooLarge);
        }
        if self.pending.len() + self.active_members() >= MAX_BATCH_MEMBERS {
            return Err(GroupError::Full);
        }
        let flush_at = self
            .next_flush_at
            .unwrap_or_else(|| now.saturating_add(COALESCE_WINDOW_SECONDS));
        self.pending.push(member.clone());
        self.next_flush_at = Some(flush_at);
        self.seen_receipt_ids.push(member.receipt_id);
        if serde_json::to_vec(self)
            .map_err(|_| GroupError::Invalid)?
            .len()
            > MAX_GROUP_BYTES
        {
            self.pending.pop();
            self.seen_receipt_ids.pop();
            return Err(GroupError::TooLarge);
        }
        if self.seen_receipt_ids.len() > MAX_SEEN_RECEIPTS {
            let excess = self.seen_receipt_ids.len() - MAX_SEEN_RECEIPTS;
            self.seen_receipt_ids.drain(..excess);
        }
        self.updated_at = now;
        Ok(EnqueueResult::Added { flush_at })
    }

    pub(super) fn due(&self, now: i64) -> bool {
        self.active.is_none()
            && !self.pending.is_empty()
            && self.next_flush_at.is_some_and(|flush_at| flush_at <= now)
    }

    pub(super) fn begin_batch(&mut self, now: i64) -> Option<Batch> {
        if !self.due(now) {
            return None;
        }
        self.generation = self.generation.saturating_add(1);
        let batch = Batch {
            batch_id: super::identity("execution-batch", &(&self.group_id, self.generation)),
            generation: self.generation,
            started_at: now,
            admission_id: None,
            launch_lease_until: now.saturating_add(60),
            members: std::mem::take(&mut self.pending),
            metadata: BTreeMap::new(),
        };
        self.active = Some(batch.clone());
        self.next_flush_at = None;
        self.last_status = "running".into();
        self.last_batch_id = Some(batch.batch_id.clone());
        self.updated_at = now;
        Some(batch)
    }

    pub(super) fn attach_admission(
        &mut self,
        batch_id: &str,
        admission_id: String,
        metadata: BTreeMap<String, String>,
        now: i64,
    ) -> Result<(), GroupError> {
        let Some(batch) = self.active.as_mut() else {
            return Err(GroupError::WrongBatch);
        };
        if batch.batch_id != batch_id {
            return Err(GroupError::WrongBatch);
        }
        batch.admission_id = Some(admission_id.clone());
        batch.metadata = metadata;
        self.last_admission_id = Some(admission_id);
        self.updated_at = now;
        Ok(())
    }

    pub(super) fn claim_active_launch(&mut self, now: i64) -> Option<Batch> {
        let batch = self.active.as_mut()?;
        if batch.admission_id.is_some() || batch.launch_lease_until > now {
            return None;
        }
        batch.launch_lease_until = now.saturating_add(60);
        self.updated_at = now;
        Some(batch.clone())
    }

    pub(super) fn finish_batch(
        &mut self,
        batch_id: &str,
        status: &str,
        response_id: Option<String>,
        result: Option<TaskResult>,
        now: i64,
    ) -> Result<(), GroupError> {
        if self
            .active
            .as_ref()
            .is_none_or(|batch| batch.batch_id != batch_id)
        {
            return Err(GroupError::WrongBatch);
        }
        self.active = None;
        self.last_status = status.into();
        self.last_response_id = response_id;
        self.result = result;
        self.next_flush_at =
            (!self.pending.is_empty()).then_some(now.saturating_add(COALESCE_WINDOW_SECONDS));
        self.updated_at = now;
        Ok(())
    }

    pub(super) fn active_members(&self) -> usize {
        self.active.as_ref().map_or(0, |batch| batch.members.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn group() -> ExecutionGroup {
        ExecutionGroup::new(
            "group-1".into(),
            "owner-1".into(),
            "app-1".into(),
            "channel-1".into(),
            "record-1".into(),
            100,
        )
    }

    fn member(receipt_id: &str, repository_id: u64) -> Member {
        Member {
            receipt_id: receipt_id.into(),
            delivery_id: format!("delivery-{receipt_id}"),
            event: "issues".into(),
            action: Some("opened".into()),
            installation_id: 7,
            authority_revision: 1,
            repository_id,
            payload: json!({"issue":{"id":1,"number":2}}),
        }
    }

    #[test]
    fn first_event_opens_a_coalesce_window() {
        let mut group = group();
        assert_eq!(
            group.enqueue(member("receipt-1", 10), 100),
            Ok(EnqueueResult::Added { flush_at: 130 })
        );
        assert!(!group.due(129));
        assert!(group.due(130));
    }

    #[test]
    fn events_before_deadline_share_one_batch() {
        let mut group = group();
        group.enqueue(member("receipt-1", 10), 100).unwrap();
        group.enqueue(member("receipt-2", 11), 110).unwrap();
        let batch = group.begin_batch(130).unwrap();
        assert_eq!(batch.members.len(), 2);
        assert_eq!(batch.generation, 1);
        assert!(group.active.is_some());
        assert!(group.pending.is_empty());
    }

    #[test]
    fn events_during_execution_are_reserved_for_the_next_batch() {
        let mut group = group();
        group.enqueue(member("receipt-1", 10), 100).unwrap();
        let first = group.begin_batch(130).unwrap();
        group.enqueue(member("receipt-2", 11), 131).unwrap();
        assert!(!group.due(160));
        group
            .finish_batch(&first.batch_id, "succeeded", None, None, 140)
            .unwrap();
        assert!(!group.due(169));
        assert!(group.due(170));
        let second = group.begin_batch(170).unwrap();
        assert_eq!(second.generation, 2);
        assert_eq!(second.members.len(), 1);
    }

    #[test]
    fn duplicate_receipt_is_idempotent_inside_a_group() {
        let mut group = group();
        group.enqueue(member("receipt-1", 10), 100).unwrap();
        assert_eq!(
            group.enqueue(member("receipt-1", 10), 101),
            Ok(EnqueueResult::Duplicate)
        );
        assert_eq!(group.pending.len(), 1);
    }

    #[test]
    fn an_active_batch_survives_restart_as_recovery_state() {
        let mut group = group();
        group.enqueue(member("receipt-1", 10), 100).unwrap();
        let batch = group.begin_batch(130).unwrap();
        let restored: ExecutionGroup =
            serde_json::from_value(serde_json::to_value(&group).unwrap()).unwrap();
        assert_eq!(restored.active.as_ref().unwrap().batch_id, batch.batch_id);
        assert!(restored.pending.is_empty());
    }

    #[test]
    fn an_expired_launch_lease_can_be_claimed_after_restart() {
        let mut group = group();
        group.enqueue(member("receipt-1", 10), 100).unwrap();
        let batch = group.begin_batch(130).unwrap();
        assert!(group.claim_active_launch(150).is_none());
        assert_eq!(
            group.claim_active_launch(191).unwrap().batch_id,
            batch.batch_id
        );
        assert_eq!(group.active.unwrap().launch_lease_until, 251);
    }
}
