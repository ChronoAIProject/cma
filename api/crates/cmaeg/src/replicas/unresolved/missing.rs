use super::*;
use crate::replicas::partition_model::{item_id, ITEM_KIND};
use mongodb::bson::Document;

pub(super) struct MissingSelected {
    pub head: Head,
    pub candidate: Candidate,
    pub classification: &'static str,
    pub work_record: Option<&'static str>,
}
impl MissingSelected {
    pub fn snapshot(&self) -> serde_json::Value {
        serde_json::json!({"head": self.head, "item": self.classification, "work_record": self.work_record})
    }
    pub fn refusal(&self, code: &'static str) -> Failure {
        Failure::refuse(code, serde_json::json!(self.candidate))
    }
}

fn expires(row: &Document) -> Result<i64, Error> {
    row.get_datetime("expires_at")
        .map(|value| value.timestamp_millis())
        .map_err(|_| Error::Corrupt)
}

impl Operator {
    pub(super) async fn select_missing(
        &self,
        binding: &Binding,
        head: &Head,
    ) -> Result<Option<MissingSelected>, Failure> {
        let store = &self.partitions.store;
        let now = store.clock().await?.millis;
        let key = item_id(&binding.head_work);
        let mismatch = || Failure::refuse("missing_item_mismatch", serde_json::json!(binding));
        let (classification, work_record) = match store.raw(&key, true).await? {
            None => {
                let work_record = match store.raw(&binding.head_work, true).await? {
                    None => "absent",
                    Some(row) => {
                        if expires(&row).map_err(|_| mismatch())? > now {
                            return Err(Failure::refuse(
                                "missing_item_plan_live",
                                serde_json::json!(binding),
                            ));
                        }
                        "expired"
                    }
                };
                ("ttl_deleted", Some(work_record))
            }
            Some(row) => {
                let expiry = expires(&row).map_err(|_| mismatch())?;
                if expiry > now {
                    return Ok(None);
                }
                let owner = row.get_str("owner").map_err(|_| mismatch())?.to_owned();
                let (item, _) = store
                    .decode::<Item>(&key, true, row)
                    .map_err(|_| mismatch())?;
                if item.kind != ITEM_KIND
                    || item.partition != binding.partition
                    || item.work_id != binding.head_work
                    || item.identity.owner != head.owner
                    || owner != head.owner
                    || item.retained_until.checked_mul(1000) != Some(expiry)
                {
                    return Err(mismatch());
                }
                if let Some(root) = store.raw(&item.identity.admission_id(), true).await? {
                    if expires(&root).map_err(|_| mismatch())? > now {
                        return Err(Failure::refuse(
                            "missing_item_plan_live",
                            serde_json::json!(binding),
                        ));
                    }
                }
                ("expired", None)
            }
        };
        // Only connection-store heads survive item retention. Never promote an
        // expiring-store head into permanent storage during operator retirement.
        let row = store
            .raw(&binding.partition, false)
            .await?
            .ok_or(Error::Lost)?;
        if row
            .get("expires_at")
            .is_some_and(|value| !matches!(value, mongodb::bson::Bson::Null))
        {
            return Err(Failure::refuse(
                "missing_item_head_expiring",
                serde_json::json!(binding),
            ));
        }
        let lease_live = head.authority.as_ref().is_some_and(|a| a.until_ms > now);
        Ok(Some(MissingSelected {
            head: head.clone(),
            classification,
            work_record,
            candidate: Candidate {
                custody: "dispatch_fifo".into(),
                binding: binding.clone(),
                partition_hash: crate::replicas::diagnostics::partition_hash(&binding.partition),
                target: binding.head_work.clone(),
                state: "missing_item_expired".into(),
                age_seconds: 0,
                queued_effect_count: 0,
                outstanding_quarantined_effects: 0,
                rollback_safe_after: utc(now.div_euclid(1000))?,
                lease_live,
                guard_live: false,
                eligible: !lease_live,
            },
        }))
    }

    pub(super) fn missing_digest(
        &self,
        req: &Request,
        selected: &MissingSelected,
    ) -> Result<String, Error> {
        hash(&(
            &req.binding,
            &req.request_id,
            &req.reason,
            selected.snapshot(),
        ))
    }

    pub(super) async fn queue_items(&self, head: &Head) -> Result<Vec<QueueItem>, Error> {
        let store = &self.partitions.store;
        let now = store.clock().await?.millis;
        let mut items = Vec::new();
        for work in head.queue.iter().skip(1).take(32) {
            let item = match store.raw(&item_id(work), true).await? {
                None => "absent",
                Some(row) if expires(&row)? <= now => "expired",
                Some(_) => "present",
            };
            items.push(QueueItem {
                work: work.clone(),
                item,
            });
        }
        Ok(items)
    }

    pub(super) async fn preview_missing(
        &self,
        req: &Request,
        selected: &MissingSelected,
    ) -> Result<Preview, Failure> {
        Ok(Preview {
            queue_depth: selected.head.queue.len(),
            queue_items: self.queue_items(&selected.head).await?,
            item_classification: Some(selected.classification),
            execution_version: 0,
            guard_id: String::new(),
            guard_version: 0,
            item_version: 0,
            queued_effect_ids: Vec::new(),
            evidence_sha256: hash(&selected.snapshot())?,
            candidate: selected.candidate.clone(),
            plan_digest: self.missing_digest(req, selected)?,
            changes: vec!["record_missing_item_retired", "advance_frontier"],
            members: self
                .partitions
                .membership
                .capabilities(&self.partitions.store)
                .await?,
        })
    }
}
