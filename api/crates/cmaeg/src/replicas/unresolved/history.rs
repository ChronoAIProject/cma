//! Project expired audits out of read-only views; apply persists pruning with CAS.
use super::super::partition_model::item_id;
use super::*;
use std::collections::{BTreeMap, BTreeSet};

fn deadline(value: &str) -> Result<i64, Error> {
    time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
        .map(|value| value.unix_timestamp())
        .map_err(|_| Error::Corrupt)
}
impl Operator {
    async fn index(&self) -> Result<(Index, u64), Error> {
        let deployment = &self.partitions.membership.deployment;
        let value = self
            .partitions
            .store
            .read::<Index>(
                &identifier("replica-abandonment-index-v1", deployment),
                false,
            )
            .await?
            .unwrap_or((
                Index {
                    kind: "replica_abandonment_index_v1".into(),
                    deployment: deployment.clone(),
                    audits: vec![],
                    deadlines: BTreeMap::new(),
                    rollback_safe_after: None,
                },
                0,
            ));
        if value.0.kind != "replica_abandonment_index_v1"
            || value.0.deployment != *deployment
            || value.0.audits.len() > 512
            || value.0.deadlines.len() != value.0.audits.len()
            || value.0.audits.iter().collect::<BTreeSet<_>>().len() != value.0.audits.len()
            || value
                .0
                .deadlines
                .keys()
                .any(|id| !value.0.audits.contains(id))
        {
            return Err(Error::Corrupt);
        }
        Ok(value)
    }
    pub(super) async fn active_index(&self) -> Result<(Index, u64, Vec<(String, Receipt)>), Error> {
        let (mut index, version) = self.index().await?;
        let now = self.partitions.store.clock().await?.millis.div_euclid(1000);
        let mut records = Vec::new();
        let mut deadlines = BTreeMap::new();
        for id in &index.audits {
            let until = *index.deadlines.get(id).ok_or(Error::Corrupt)?;
            if until <= now {
                continue;
            }
            let (audit, audit_version) = self
                .partitions
                .store
                .read::<Receipt>(id, true)
                .await?
                .ok_or(Error::Expired)?;
            if audit_version != 1
                || audit.kind != "replica_unknown_abandonment_v1"
                || audit.disposition != "unknown_abandoned"
                || identifier("replica-abandonment-v1", &audit.request.request_id) != *id
                || deadline(&audit.rollback_safe_after)? != until
            {
                return Err(Error::Corrupt);
            }
            deadlines.insert(id.clone(), until);
            records.push((id.clone(), audit));
        }
        index.audits = records.iter().map(|(id, _)| id.clone()).collect();
        index.rollback_safe_after = deadlines.values().max().copied().map(utc).transpose()?;
        index.deadlines = deadlines;
        Ok((index, version, records))
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
    async fn effect_counts(
        &self,
        targets: &BTreeSet<String>,
    ) -> Result<BTreeMap<String, usize>, Error> {
        let mut counts = targets
            .iter()
            .map(|target| (target.clone(), 0))
            .collect::<BTreeMap<_, _>>();
        if targets.is_empty() {
            return Ok(counts);
        }
        let now = self.partitions.store.clock().await?.millis.div_euclid(1000);
        let mut cursor = None;
        loop {
            let rows = self
                .partitions
                .store
                .effect_history(&self.partitions.membership.deployment, cursor.as_deref())
                .await?;
            let len = rows.len();
            for (id, root, _) in rows {
                cursor = Some(id.clone());
                if root["kind"] != "routed_delayed_reply_v1" {
                    continue;
                }
                let target = root["target"].as_str().ok_or(Error::Corrupt)?;
                let Some(count) = counts.get_mut(target) else {
                    continue;
                };
                if root["expires_at"].as_i64().ok_or(Error::Corrupt)? <= now {
                    continue;
                }
                let retired = self
                    .partitions
                    .store
                    .read::<Item>(&item_id(&id), true)
                    .await?
                    .is_some_and(|(item, _)| item.done);
                if !retired || root["state"]["phase"] != "settled" {
                    *count += 1;
                }
            }
            if len < 128 {
                return Ok(counts);
            }
        }
    }
    async fn media_count(&self, target: &str, plan: &AdmittedPlan) -> Result<usize, Error> {
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
            return Ok(usize::from(!retired || !(inactive || refused)));
        }
        Ok(0)
    }
    pub(super) async fn outstanding(
        &self,
        target: &str,
        plan: &AdmittedPlan,
    ) -> Result<usize, Error> {
        let counts = self.effect_counts(&BTreeSet::from([target.into()])).await?;
        Ok(counts[target] + self.media_count(target, plan).await?)
    }
    pub async fn inspect(&self) -> Result<Inspection, Error> {
        self.inspect_bounded(4096).await
    }
    #[cfg(test)]
    pub(crate) async fn inspect_with_limit(&self, limit: usize) -> Result<Inspection, Error> {
        self.inspect_bounded(limit).await
    }
    async fn inspect_bounded(&self, limit: usize) -> Result<Inspection, Error> {
        let members = self
            .partitions
            .membership
            .capabilities(&self.partitions.store)
            .await?;
        let scan = self.scan_heads(limit.min(4096)).await?;
        let (index, _, records) = self.active_index().await?;
        let targets = records
            .iter()
            .map(|(_, audit)| audit.target.clone())
            .collect::<BTreeSet<_>>();
        if targets.len() != records.len() {
            return Err(Error::Corrupt);
        }
        let mut counts = self.effect_counts(&targets).await?;
        let mut abandonments = Vec::new();
        for (id, audit) in records {
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
                .map_err(admission_error)?
                .ok_or(Error::Expired)?;
            if plan.identity.owner != audit.owner || item.target != audit.target {
                return Err(Error::Corrupt);
            }
            let count = counts.get_mut(&audit.target).ok_or(Error::Corrupt)?;
            *count += self.media_count(&audit.target, &plan).await?;
            abandonments.push(Abandonment {
                audit: id,
                target: audit.target,
                rollback_safe_after: audit.rollback_safe_after,
                outstanding_effects: *count,
            });
        }
        let mut candidates = scan.candidates;
        for candidate in &mut candidates {
            candidate.outstanding_quarantined_effects =
                counts.get(&candidate.target).copied().unwrap_or(0);
        }
        Ok(Inspection {
            candidates,
            skipped: scan.skipped,
            truncated: scan.truncated,
            abandonments,
            rollback_safe_after: index.rollback_safe_after,
            outstanding_quarantined_effects: counts.values().sum(),
            members,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replicas::{mongo_fixture::Fixture, store::Write};

    #[tokio::test]
    #[ignore = "requires authenticated disposable Mongo replica set"]
    async fn index_requires_each_deadline_and_does_not_infer_expiry_from_the_maximum() {
        for fault in [
            "missing_field",
            "missing_entry",
            "missing_unexpired_receipt",
        ] {
            let fixture = Fixture::new().await;
            let node = fixture.node(1).await;
            node.seed_expired_operator_index(1).await;
            let (index, version) = node.operator_index().await;
            let id = identifier("replica-abandonment-index-v1", &index.deployment);
            let mut value = serde_json::to_value(&index).unwrap();
            match fault {
                "missing_field" => {
                    value.as_object_mut().unwrap().remove("deadlines");
                }
                "missing_entry" => value["deadlines"] = serde_json::json!({}),
                _ => {
                    let now = node
                        .partitions
                        .store
                        .clock()
                        .await
                        .unwrap()
                        .millis
                        .div_euclid(1000);
                    value["deadlines"][&index.audits[0]] = serde_json::json!(now + 3600);
                    // The stored maximum remains in the past and must not hide
                    // the absent receipt for this unexpired indexed entry.
                }
            }
            node.partitions
                .store
                .write(
                    &id,
                    &index.deployment,
                    &value,
                    Write {
                        expected: Some(version),
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
            let operator = Operator::new(node.partitions.clone(), node.plans(), node.journal());
            let error = operator.inspect().await.err();
            drop(operator);
            drop(node);
            fixture.cleanup().await;
            assert_eq!(
                error,
                Some(if fault == "missing_unexpired_receipt" {
                    Error::Expired
                } else {
                    Error::Corrupt
                }),
                "{fault}"
            );
        }
    }
}
