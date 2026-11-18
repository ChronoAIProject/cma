use super::super::partition_model::item_id;
use super::*;
use crate::replicas::diagnostics::{Class, ScanError, Stage};

impl Operator {
    pub(super) async fn select(
        &self,
        binding: &Binding,
        counts: bool,
    ) -> Result<Selected, Failure> {
        let store = &self.partitions.store;
        let (head, version, head_retention) = store
            .head_record(&binding.partition)
            .await
            .map_err(|e| {
                Failure::Head(ScanError::new(e, Stage::HeadRead, Some(&binding.partition)))
            })?
            .ok_or_else(|| {
                Failure::refuse(
                    "binding_version_mismatch",
                    serde_json::json!({"requested":binding,"actual":null}),
                )
            })?;
        head.validate(
            &binding.partition,
            &self.partitions.membership.deployment,
            &head.owner,
        )
        .map_err(|e| {
            Failure::Head(ScanError::new(
                e,
                Stage::HeadValidate,
                Some(&binding.partition),
            ))
        })?;
        if version != binding.head_version || head.queue.front() != Some(&binding.head_work) {
            return Err(Failure::refuse(
                "binding_version_mismatch",
                serde_json::json!({"requested":binding,"actual":{"partition":head.id,"head_work":head.queue.front(),"head_version":version}}),
            ));
        }
        let loaded = match crate::replicas::head_evidence::load(
            &self.partitions,
            &self.plans,
            &binding.partition,
            &head,
            false,
        )
        .await
        {
            Ok(loaded) => loaded.ok_or(Error::Lost)?,
            Err(error) if error.class == Class::MissingItem && error.stage == Stage::ItemRead => {
                if head_retention.is_some() {
                    return Err(Failure::refuse(
                        "missing_item_head_expiring",
                        serde_json::json!(binding),
                    ));
                }
                if let Some(selected) = self.select_missing(binding, &head).await? {
                    return Ok(Selected::Missing(Box::new(selected)));
                }
                return Err(Failure::Head(error));
            }
            Err(error) => return Err(Failure::Head(error)),
        };
        let crate::replicas::head_evidence::Loaded {
            item,
            item_version,
            plan,
            target_index,
        } = loaded;
        let fail = |error: Error, stage| {
            Failure::Head(
                ScanError::new(error, stage, Some(&binding.partition))
                    .aged((self.partitions.membership.now_seconds() - plan.admitted_at).max(0)),
            )
        };
        let target = &plan.targets[target_index];
        let lane_id =
            super::super::delayed_lane::id(&self.partitions.membership.deployment, &plan, target);
        if head_retention.is_some() != (binding.partition == lane_id) {
            return Err(fail(Error::Corrupt, Stage::HeadValidate));
        }
        if item.effect
            && self
                .partitions
                .validate_effect_item(&plan, target, &item.work_id, &item)
                .map_err(|e| fail(e, Stage::ItemCheck))?
                != head_retention
        {
            return Err(fail(Error::Corrupt, Stage::ItemCheck));
        }
        let lane = if binding.partition != lane_id {
            store
                .head_record(&lane_id)
                .await
                .map_err(|e| fail(e, Stage::HeadRead))?
                .map(|(head, version, retained)| (head, version, retained.unwrap_or(0)))
        } else {
            None
        };
        if lane.as_ref().is_some_and(|(head, _, until)| {
            *until != super::super::delayed_lane::retention(&plan, target)
                || head
                    .validate(
                        &lane_id,
                        &self.partitions.membership.deployment,
                        &plan.identity.owner,
                    )
                    .is_err()
        }) {
            return Err(fail(Error::Corrupt, Stage::HeadValidate));
        }
        let evidence = crate::turns::routed::operator::inspect(&self.journal, &plan, target)
            .await
            .map_err(|e| fail(execution_error(e), Stage::Evidence))?;
        let now = store
            .clock()
            .await
            .map_err(|e| fail(e, Stage::Clock))?
            .millis;
        let (effect, mut state, mut eligible) = if item.effect {
            let (value, version, label, eligible) = crate::turns::routed::operator::effect(
                &self.journal,
                &self.plans,
                &plan,
                target,
                &item.work_id,
            )
            .await
            .map_err(|e| fail(execution_error(e), Stage::Effect))?;
            (Some((value, version)), label, eligible)
        } else {
            (None, evidence.state.clone(), evidence.eligible)
        };
        let lease_live = head.authority.as_ref().is_some_and(|a| a.until_ms > now)
            || lane.as_ref().is_some_and(|(head, _, _)| {
                head.authority.as_ref().is_some_and(|a| a.until_ms > now)
            });
        if state.ends_with("abandoned_sending") && lease_live {
            state = "effect_sending".into();
            eligible = false;
        }
        let mut queued_effect_count = 0;
        let mut queued_effect_ids = Vec::new();
        for work in head.queue.iter().skip(1) {
            let (queued, _) = store
                .read::<Item>(&item_id(work), true)
                .await
                .map_err(|e| fail(e, Stage::QueueRead))?
                .ok_or_else(|| {
                    Failure::Head(
                        ScanError::new(
                            crate::replicas::diagnostics::Class::MissingItem,
                            Stage::QueueRead,
                            Some(&binding.partition),
                        )
                        .aged((now / 1000 - plan.admitted_at).max(0)),
                    )
                })?;
            if queued.effect {
                queued_effect_count += 1;
                queued_effect_ids.push(queued.work_id.clone());
            }
        }
        let candidate = Candidate {
            custody: if head_retention.is_some() {
                "delayed_lane"
            } else {
                "dispatch_fifo"
            }
            .into(),
            binding: binding.clone(),
            partition_hash: crate::replicas::diagnostics::partition_hash(&binding.partition),
            target: target.id.clone(),
            state,
            eligible,
            age_seconds: (now / 1000 - plan.admitted_at).max(0),
            queued_effect_count,
            outstanding_quarantined_effects: if counts
                && self
                    .journal
                    .quarantined(&plan, target)
                    .await
                    .map_err(|e| fail(execution_error(e), Stage::Evidence))?
            {
                self.outstanding(&target.id, &plan)
                    .await
                    .map_err(|e| fail(e, Stage::Evidence))?
            } else {
                0
            },
            rollback_safe_after: utc(target
                .launch
                .event
                .expires_at
                .saturating_add(cmaeg_core::seal::REPLY_TARGET_TTL))
            .map_err(|e| fail(e, Stage::Evidence))?,
            lease_live,
            guard_live: evidence.live_until > now / 1000,
        };
        Ok(Selected::Normal(Box::new(NormalSelected {
            head,
            head_retention,
            lane,
            item,
            item_version,
            plan,
            target_index,
            evidence,
            effect,
            candidate,
            queued_effect_ids,
        })))
    }
    pub(super) fn digest(&self, req: &Request, selected: &NormalSelected) -> Result<String, Error> {
        hash(&(
            &req.binding,
            &req.request_id,
            &req.reason,
            selected.evidence.execution_version,
            selected.evidence.guard_version,
            selected.item_version,
            &selected.effect,
            &selected.head,
            &selected.lane,
        ))
    }
    pub async fn preview(&self, req: &Request) -> Result<Preview, Failure> {
        self.validate_request(req)?;
        let selected = match self.select(&req.binding, true).await? {
            Selected::Normal(selected) => selected,
            Selected::Missing(selected) => return self.preview_missing(req, &selected).await,
        };
        let members = self
            .partitions
            .membership
            .capabilities(&self.partitions.store)
            .await?;
        let queue_items = self.queue_items(&selected.head).await?;
        Ok(Preview {
            queue_depth: selected.head.queue.len(),
            queue_items,
            item_classification: None,
            execution_version: selected.evidence.execution_version,
            guard_id: selected.evidence.guard_id.clone(),
            guard_version: selected.evidence.guard_version,
            item_version: selected.item_version,
            queued_effect_ids: selected.queued_effect_ids.clone(),
            evidence_sha256: hash(&Snapshot {
                execution_version: selected.evidence.execution_version,
                guard_version: selected.evidence.guard_version,
                effect_version: selected.effect.as_ref().map(|v| v.1),
                execution: selected.evidence.execution.clone(),
                guard: selected.evidence.guard.clone(),
                effect: selected.effect.as_ref().map(|v| v.0.clone()),
            })?,
            plan_digest: self.digest(req, &selected)?,
            candidate: selected.candidate,
            changes: vec![
                "record_unknown_abandoned",
                "quarantine_target",
                "close_reply_guard",
                "retire_head_item",
                "advance_frontier",
            ],
            members,
        })
    }
    pub(super) fn validate_request(&self, req: &Request) -> Result<(), Error> {
        if uuid::Uuid::parse_str(&req.request_id).is_err()
            || req.reason.trim().is_empty()
            || req.reason.len() > 1024
            || req.reason.chars().any(char::is_control)
        {
            return Err(Error::Corrupt);
        }
        Ok(())
    }
}
