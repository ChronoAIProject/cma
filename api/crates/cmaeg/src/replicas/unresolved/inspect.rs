use super::super::partition_model::item_id;
use super::*;
use crate::records::mongo::ReplicaKind;

impl Operator {
    pub(super) async fn select(&self, binding: &Binding, counts: bool) -> Result<Selected, Error> {
        let store = &self.partitions.store;
        let (head, version) = store
            .read::<Head>(&binding.partition, false)
            .await?
            .ok_or(Error::Lost)?;
        head.validate(
            &binding.partition,
            &self.partitions.membership.deployment,
            &head.owner,
        )?;
        if version != binding.head_version || head.queue.front() != Some(&binding.head_work) {
            return Err(Error::Busy);
        }
        let (item, item_version) = store
            .read::<Item>(&item_id(&binding.head_work), true)
            .await?
            .ok_or(Error::Expired)?;
        if item.work_id != binding.head_work || item.partition != binding.partition || item.done {
            return Err(Error::Corrupt);
        }
        let plan = self
            .plans
            .load(&item.identity)
            .await
            .map_err(|_| Error::Corrupt)?
            .ok_or(Error::Expired)?;
        let target_index = plan
            .targets
            .iter()
            .position(|t| t.id == item.target)
            .ok_or(Error::Corrupt)?;
        if item.request_hash != plan.request_hash || head.owner != plan.identity.owner {
            return Err(Error::Corrupt);
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
    pub(super) async fn index(&self) -> Result<(Index, u64), Error> {
        let deployment = &self.partitions.membership.deployment;
        let id = identifier("replica-abandonment-index-v1", deployment);
        let value = self
            .partitions
            .store
            .read::<Index>(&id, false)
            .await?
            .unwrap_or((
                Index {
                    kind: "replica_abandonment_index_v1".into(),
                    deployment: deployment.clone(),
                    audits: Vec::new(),
                    rollback_safe_after: None,
                },
                0,
            ));
        if value.0.kind != "replica_abandonment_index_v1"
            || value.0.deployment != *deployment
            || value.0.audits.len() > 512
        {
            return Err(Error::Corrupt);
        }
        Ok(value)
    }
    pub async fn audit(&self, request: &str) -> Result<Receipt, Error> {
        uuid::Uuid::parse_str(request).map_err(|_| Error::Corrupt)?;
        self.partitions
            .store
            .read::<Receipt>(&identifier("replica-abandonment-v1", request), true)
            .await?
            .map(|v| v.0)
            .ok_or(Error::Lost)
    }
    pub(super) async fn outstanding(
        &self,
        target: &str,
        plan: &AdmittedPlan,
    ) -> Result<usize, Error> {
        let mut count = 0;
        let mut after = None;
        let now = self.partitions.store.clock().await?.millis / 1000;
        // Active roots include payloads admitted before enqueue acknowledgement.
        loop {
            let rows = self
                .partitions
                .store
                .effect_history(&self.partitions.membership.deployment, after.as_deref())
                .await?;
            let len = rows.len();
            for (id, root, _) in rows {
                after = Some(id.clone());
                if root["kind"] != "routed_delayed_reply_v1"
                    || root["target"] != target
                    || root["expires_at"].as_i64().ok_or(Error::Corrupt)? <= now
                {
                    continue;
                }
                let retired = self
                    .partitions
                    .store
                    .read::<Item>(&item_id(&id), true)
                    .await?
                    .is_some_and(|(item, _)| item.done);
                // Previously completed occurrences were never quarantined.
                // Newly quarantined queued roots settle only with a refusal.
                let settled = root["state"]["phase"] == "settled";
                if !retired || !settled {
                    count += 1;
                }
            }
            if len < 128 {
                break;
            }
        }
        let retained = plan
            .targets
            .iter()
            .find(|t| t.id == target)
            .ok_or(Error::Corrupt)?;
        if let Some((work, inactive, refused)) = crate::turns::routed::operator::media_custody(
            &self.journal,
            &self.plans,
            plan,
            retained,
        )
        .await
        .map_err(execution_error)?
        {
            let retired = self
                .partitions
                .store
                .read::<Item>(&item_id(&work), true)
                .await?
                .is_some_and(|(item, _)| item.done);
            if !retired || !(inactive || refused) {
                count += 1;
            }
        }
        Ok(count)
    }
    pub async fn inspect(&self) -> Result<Inspection, Error> {
        let members = self
            .partitions
            .membership
            .capabilities(&self.partitions.store)
            .await?;
        let candidates = self.head_candidates(true).await?;
        let (index, _) = self.index().await?;
        let mut abandonments = Vec::new();
        let mut maximum = index.rollback_safe_after.clone();
        let mut outstanding = 0;
        for id in index.audits {
            let Some((audit, _)) = self.partitions.store.read::<Receipt>(&id, true).await? else {
                continue;
            };
            let (item, _) = self
                .partitions
                .store
                .read::<Item>(&item_id(&audit.request.binding.head_work), true)
                .await?
                .ok_or(Error::Expired)?;
            let plan = self
                .plans
                .load(&item.identity)
                .await
                .map_err(|_| Error::Corrupt)?
                .ok_or(Error::Expired)?;
            if plan.identity.owner != audit.owner || item.target != audit.target {
                return Err(Error::Corrupt);
            }
            let count = self.outstanding(&audit.target, &plan).await?;
            maximum = Some(
                maximum.map_or(audit.rollback_safe_after.clone(), |m: String| {
                    m.max(audit.rollback_safe_after.clone())
                }),
            );
            outstanding += count;
            abandonments.push(Abandonment {
                audit: id,
                target: audit.target,
                rollback_safe_after: audit.rollback_safe_after,
                outstanding_effects: count,
            });
        }
        Ok(Inspection {
            candidates,
            abandonments,
            rollback_safe_after: maximum,
            outstanding_quarantined_effects: outstanding,
            members,
        })
    }
    async fn head_candidates(&self, counts: bool) -> Result<Vec<Candidate>, Error> {
        let mut candidates = Vec::new();
        let mut scanned = 0usize;
        let mut cursor = None;
        loop {
            let rows = self
                .partitions
                .store
                .active::<Head>(
                    ReplicaKind::Partition,
                    &self.partitions.membership.deployment,
                    cursor.as_deref(),
                    128,
                )
                .await?;
            let len = rows.len();
            for (id, head, version) in rows {
                scanned += 1;
                if scanned > 4096 {
                    return Err(Error::Capacity);
                }
                cursor = Some(id.clone());
                if let Some(work) = head.queue.front() {
                    let selected = self
                        .select(
                            &Binding {
                                partition: id,
                                head_work: work.clone(),
                                head_version: version,
                            },
                            counts,
                        )
                        .await?;
                    if selected.candidate.eligible {
                        candidates.push(selected.candidate);
                    }
                }
            }
            if len < 128 {
                break;
            }
        }
        Ok(candidates)
    }
    pub async fn observe_heads(&self) -> Result<(), Error> {
        let candidates = self.head_candidates(false).await?;
        metrics::gauge!("cmaeg_replica_retained_uncertain_heads").set(candidates.len() as f64);
        metrics::gauge!("cmaeg_replica_retained_uncertain_oldest_age_seconds")
            .set(candidates.iter().map(|c| c.age_seconds).max().unwrap_or(0) as f64);
        let mut seen = self.seen.lock().map_err(|_| Error::Lost)?;
        for c in &candidates {
            if seen.insert(c.binding.head_work.clone()) {
                tracing::warn!(partition=%c.binding.partition, work=%c.binding.head_work, state=%c.state, age_seconds=c.age_seconds, "partition head retained uncertain; operator inspection required");
            }
        }
        seen.retain(|work| candidates.iter().any(|c| &c.binding.head_work == work));
        Ok(())
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
    pub async fn preview(&self, req: &Request) -> Result<Preview, Error> {
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
