use super::super::partition_model::item_id;
use super::*;

impl Operator {
    pub(super) async fn select(
        &self,
        binding: &Binding,
        counts: bool,
    ) -> Result<Selected, Failure> {
        let store = &self.partitions.store;
        let (head, version) = store
            .read::<Head>(&binding.partition, false)
            .await?
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
        )?;
        if version != binding.head_version || head.queue.front() != Some(&binding.head_work) {
            return Err(Failure::refuse(
                "binding_version_mismatch",
                serde_json::json!({"requested":binding,"actual":{"partition":head.id,"head_work":head.queue.front(),"head_version":version}}),
            ));
        }
        let (item, item_version) = store
            .read::<Item>(&item_id(&binding.head_work), true)
            .await?
            .ok_or(Error::Expired)?;
        if item.work_id != binding.head_work || item.partition != binding.partition || item.done {
            return Err(Error::Corrupt.into());
        }
        let plan = self
            .plans
            .load(&item.identity)
            .await
            .map_err(admission_error)?
            .ok_or(Error::Expired)?;
        let target_index = plan
            .targets
            .iter()
            .position(|t| t.id == item.target)
            .ok_or(Error::Corrupt)?;
        if item.request_hash != plan.request_hash || head.owner != plan.identity.owner {
            return Err(Error::Corrupt.into());
        }
        let target = &plan.targets[target_index];
        let evidence = crate::turns::routed::operator::inspect(&self.journal, &plan, target)
            .await
            .map_err(execution_error)?;
        let now = store.clock().await?.millis;
        let (effect, mut state, mut eligible) = if item.effect {
            let (value, version, label, eligible) = crate::turns::routed::operator::effect(
                &self.journal,
                &self.plans,
                &plan,
                target,
                &item.work_id,
            )
            .await
            .map_err(execution_error)?;
            (Some((value, version)), label, eligible)
        } else {
            (None, evidence.state.clone(), evidence.eligible)
        };
        let lease_live = head.authority.as_ref().is_some_and(|a| a.until_ms > now);
        if state.ends_with("abandoned_sending") && lease_live {
            state = "effect_sending".into();
            eligible = false;
        }
        let mut queued_effect_count = 0;
        let mut queued_effect_ids = Vec::new();
        for work in head.queue.iter().skip(1) {
            let (queued, _) = store
                .read::<Item>(&item_id(work), true)
                .await?
                .ok_or(Error::Expired)?;
            if queued.effect {
                queued_effect_count += 1;
                queued_effect_ids.push(queued.work_id.clone());
            }
        }
        let candidate = Candidate {
            binding: binding.clone(),
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
                    .map_err(execution_error)?
            {
                self.outstanding(&target.id, &plan).await?
            } else {
                0
            },
            rollback_safe_after: utc(target
                .launch
                .event
                .expires_at
                .saturating_add(cmaeg_core::seal::REPLY_TARGET_TTL))?,
            lease_live,
            guard_live: evidence.live_until > now / 1000,
        };
        Ok(Selected {
            head,
            item,
            item_version,
            plan,
            target_index,
            evidence,
            effect,
            candidate,
            queued_effect_ids,
        })
    }
    pub(super) fn digest(&self, req: &Request, selected: &Selected) -> Result<String, Error> {
        hash(&(
            &req.binding,
            &req.request_id,
            &req.reason,
            selected.evidence.execution_version,
            selected.evidence.guard_version,
            selected.item_version,
            &selected.effect,
            &selected.head,
        ))
    }
    pub async fn preview(&self, req: &Request) -> Result<Preview, Failure> {
        self.validate_request(req)?;
        let selected = self.select(&req.binding, true).await?;
        let members = self
            .partitions
            .membership
            .capabilities(&self.partitions.store)
            .await?;
        Ok(Preview {
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
            changes: [
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
