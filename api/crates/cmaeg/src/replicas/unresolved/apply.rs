use super::super::{partition_model::item_id, store::Write};
use super::*;
use crate::{
    records::mongo::{LeaseCondition, ReplicaKind, ReplicaProjection},
    turns::routed::quarantine,
};
use base64::{engine::general_purpose::STANDARD, Engine};

impl Operator {
    pub async fn apply(&self, req: &Request, authority_key_id: u8) -> Result<Receipt, Failure> {
        self.validate_request(req)?;
        let audit_id = identifier("replica-abandonment-v1", &req.request_id);
        if let Some((saved, _)) = self
            .partitions
            .store
            .read::<Receipt>(&audit_id, true)
            .await?
        {
            if hash(&saved.request)? != hash(req)? {
                return Err(Failure::refuse(
                    "request_id_reuse",
                    serde_json::json!({"existing_binding":saved.request.binding,"target":saved.target}),
                ));
            }
            return Ok(saved);
        }
        let mut selected = self.select(&req.binding, true).await?;
        if selected.candidate.lease_live {
            return Err(selected.refusal("live_lease"));
        }
        if selected.candidate.guard_live {
            return Err(selected.refusal("live_guard_attempt"));
        }
        let prior_quarantine = self
            .journal
            .quarantined(
                &selected.plan,
                &selected.plan.targets[selected.target_index],
            )
            .await
            .map_err(execution_error)?;
        if prior_quarantine {
            return Err(selected.refusal("already_quarantined"));
        }
        if !selected.candidate.eligible {
            return Err(selected.refusal("ineligible_state"));
        }
        if req.plan_digest.as_deref() != Some(self.digest(req, &selected)?.as_str()) {
            return Err(selected.refusal("plan_digest_mismatch"));
        }
        let target = &selected.plan.targets[selected.target_index];
        let members = self
            .partitions
            .membership
            .capabilities(&self.partitions.store)
            .await?;
        if members.iter().any(|m| !m.quarantine) {
            return Err(Failure::refuse(
                "cohort_incompatible",
                serde_json::json!({"members":members}),
            ));
        }
        let snapshot = Snapshot {
            execution_version: selected.evidence.execution_version,
            guard_version: selected.evidence.guard_version,
            effect_version: selected.effect.as_ref().map(|v| v.1),
            execution: selected.evidence.execution,
            guard: selected.evidence.guard.clone(),
            effect: selected.effect.as_ref().map(|v| v.0.clone()),
        };
        let bytes = serde_json::to_vec(&snapshot).map_err(|_| Error::Corrupt)?;
        let evidence_sha256 = hash(&snapshot)?;
        let mut evidence = Vec::new();
        // Immutable chunks carry no effect authority. Lost writes are reconciled
        // by exact identity by Journal::write; only the fenced audit publishes them.
        for (index, chunk) in bytes.chunks(24 * 1024).enumerate() {
            let id = identifier(
                "replica-abandonment-evidence-v1",
                &format!("{audit_id}:{evidence_sha256}:{index}"),
            );
            let chunk = serde_json::json!({"kind":"replica_abandonment_evidence_v1","audit":audit_id,"index":index,"base64":STANDARD.encode(chunk)});
            if let Some((existing, _)) = self
                .partitions
                .store
                .read::<serde_json::Value>(&id, true)
                .await?
            {
                if existing != chunk {
                    return Err(Error::Corrupt.into());
                }
            } else {
                self.partitions
                    .store
                    .write(
                        &id,
                        &selected.plan.identity.owner,
                        &chunk,
                        Write {
                            retained_until: Some(selected.plan.retained_until),
                            ..Default::default()
                        },
                    )
                    .await?;
            }
            evidence.push(id);
        }
        let now = self.partitions.store.clock().await?.millis / 1000;
        let receipt = Receipt {
            kind: "replica_unknown_abandonment_v1".into(),
            authority_key_id,
            request: req.clone(),
            target: target.id.clone(),
            disposition: "unknown_abandoned".into(),
            timestamp: utc(now)?,
            rollback_safe_after: selected.candidate.rollback_safe_after,
            prior_state: selected.candidate.state,
            evidence,
            evidence_sha256,
            owner: selected.plan.identity.owner.clone(),
            retained_until: selected.plan.retained_until,
        };
        let quarantine = quarantine::Quarantine {
            kind: quarantine::KIND.into(),
            target: target.id.clone(),
            root: selected.plan.admission_id.clone(),
            owner: receipt.owner.clone(),
            request_hash: selected.plan.request_hash.clone(),
            retained_until: receipt.retained_until,
            audit: audit_id.clone(),
            rollback_safe_after: target
                .launch
                .event
                .expires_at
                .saturating_add(cmaeg_core::seal::REPLY_TARGET_TTL),
        };
        let quarantine_id = quarantine::id(&target.id);
        let prior = self
            .partitions
            .store
            .read::<quarantine::Quarantine>(&quarantine_id, true)
            .await?;
        if prior.is_some() {
            return Err(Failure::refuse(
                "already_quarantined",
                serde_json::json!({"target":target.id,"state":receipt.prior_state}),
            ));
        }
        let (mut index, index_version, _) = self.active_index().await?;
        if index.audits.len() >= 512 {
            return Err(Error::Capacity.into());
        }
        index.audits.push(audit_id.clone());
        index
            .deadlines
            .insert(audit_id.clone(), quarantine.rollback_safe_after);
        index.rollback_safe_after = Some(
            index
                .rollback_safe_after
                .map_or(receipt.rollback_safe_after.clone(), |v| {
                    v.max(receipt.rollback_safe_after.clone())
                }),
        );
        selected.evidence.guard["closed"] = serde_json::Value::Bool(true);
        selected.item.done = true;
        selected.head.queue.pop_front();
        let expiry = selected
            .head
            .authority
            .as_ref()
            .map(|a| LeaseCondition::Expired(a.until_ms));
        selected.head.authority = None;
        let store = &self.partitions.store;
        let mut writes = vec![
            store.prepare(
                &audit_id,
                &receipt.owner,
                &receipt,
                Write {
                    retained_until: Some(receipt.retained_until),
                    ..Default::default()
                },
            )?,
            store.prepare(
                &quarantine_id,
                &receipt.owner,
                &quarantine,
                Write {
                    retained_until: Some(receipt.retained_until),
                    ..Default::default()
                },
            )?,
            store.prepare(
                &selected.evidence.guard_id,
                &receipt.owner,
                &selected.evidence.guard,
                Write {
                    retained_until: Some(receipt.retained_until),
                    expected: (selected.evidence.guard_version > 0)
                        .then_some(selected.evidence.guard_version),
                    ..Default::default()
                },
            )?,
            store.prepare(
                &item_id(&selected.item.work_id),
                &receipt.owner,
                &selected.item,
                Write {
                    retained_until: Some(receipt.retained_until),
                    expected: Some(selected.item_version),
                    ..Default::default()
                },
            )?,
            store.prepare(
                &req.binding.partition,
                &receipt.owner,
                &selected.head,
                Write {
                    expected: Some(req.binding.head_version),
                    condition: expiry,
                    projection: Some(ReplicaProjection {
                        kind: ReplicaKind::Partition,
                        deployment: self.partitions.membership.deployment.clone(),
                        active: !selected.head.queue.is_empty(),
                    }),
                    ..Default::default()
                },
            )?,
            store.prepare(
                &identifier("replica-abandonment-index-v1", &index.deployment),
                &index.deployment,
                &index,
                Write {
                    expected: (index_version > 0).then_some(index_version),
                    ..Default::default()
                },
            )?,
        ];
        if let Some((root, version)) = &selected.effect {
            if root["kind"] == "routed_delayed_reply_v1" {
                writes.push(store.prepare(
                    &selected.item.work_id,
                    &receipt.owner,
                    root,
                    Write {
                        expected: Some(*version),
                        retained_until: Some(receipt.retained_until),
                        projection: Some(ReplicaProjection {
                            kind: ReplicaKind::Effect,
                            deployment: self.partitions.membership.deployment.clone(),
                            active: false,
                        }),
                        ..Default::default()
                    },
                )?);
            }
        }
        // Recheck the cohort after evidence staging, before the single commit.
        // The runbook excludes concurrent old joins/rollouts; this is no join lock.
        let members = self.partitions.membership.capabilities(store).await?;
        if members.iter().any(|m| !m.quarantine) {
            return Err(Failure::refuse(
                "cohort_incompatible",
                serde_json::json!({"members":members}),
            ));
        }
        match store.batch(&writes).await {
            Ok(_) => Ok(receipt),
            Err(error) => match store.read::<Receipt>(&audit_id, true).await? {
                Some((saved, _)) if hash(&saved.request)? == hash(req)? => Ok(saved),
                _ => {
                    if error == Error::Busy {
                        if let Err(changed) = self.select(&req.binding, false).await {
                            if changed.code() == "binding_version_mismatch" {
                                return Err(changed);
                            }
                        }
                    }
                    Err(error.into())
                }
            },
        }
    }
}
