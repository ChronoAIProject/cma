use super::{missing::MissingSelected, *};
use crate::{
    records::mongo::{LeaseCondition, ReplicaKind, ReplicaProjection},
    replicas::store::Write,
};
use base64::{engine::general_purpose::STANDARD, Engine};

// Keep operator replay and evidence available for a month after plan retention
// has ended. This is audit retention only; it never extends work authority.
const AUDIT_RETENTION_SECONDS: i64 = 30 * 86400;

impl Operator {
    pub(super) async fn apply_missing(
        &self,
        req: &Request,
        authority_key_id: u8,
        mut selected: MissingSelected,
    ) -> Result<Receipt, Failure> {
        if selected.candidate.lease_live {
            return Err(selected.refusal("live_lease"));
        }
        if req.plan_digest.as_deref() != Some(self.missing_digest(req, &selected)?.as_str()) {
            return Err(selected.refusal("plan_digest_mismatch"));
        }
        let store = &self.partitions.store;
        let now = store.clock().await?.millis.div_euclid(1000);
        let audit_id = identifier("replica-abandonment-v1", &req.request_id);
        let snapshot = selected.snapshot();
        let evidence_sha256 = hash(&snapshot)?;
        let evidence_id = identifier(
            "replica-abandonment-evidence-v1",
            &format!("{audit_id}:{evidence_sha256}:0"),
        );
        let bytes = serde_json::to_vec(&snapshot).map_err(|_| Error::Corrupt)?;
        let evidence = serde_json::json!({
            "kind": "replica_abandonment_evidence_v1", "audit": audit_id,
            "index": 0, "base64": STANDARD.encode(bytes),
        });
        let receipt = Receipt {
            kind: "replica_unknown_abandonment_v1".into(),
            authority_key_id,
            request: req.clone(),
            target: req.binding.head_work.clone(),
            disposition: "missing_item_retired".into(),
            timestamp: utc(now)?,
            rollback_safe_after: utc(now)?,
            prior_state: "missing_item_expired".into(),
            evidence: vec![evidence_id.clone()],
            evidence_sha256,
            owner: selected.head.owner.clone(),
            retained_until: now + AUDIT_RETENTION_SECONDS,
        };
        let (mut index, version, _) = self.active_index().await?;
        if index.audits.len() >= 512 {
            return Err(Error::Capacity.into());
        }
        index.audits.push(audit_id.clone());
        // Existing readers skip this already-safe deadline before loading any
        // plan evidence. The retained receipt remains readable by request UUID.
        index.deadlines.insert(audit_id.clone(), now);
        index.rollback_safe_after = Some(
            index
                .rollback_safe_after
                .map_or(receipt.rollback_safe_after.clone(), |value| {
                    value.max(receipt.rollback_safe_after.clone())
                }),
        );
        selected.head.queue.pop_front();
        let expiry = selected
            .head
            .authority
            .as_ref()
            .map(|a| LeaseCondition::Expired(a.until_ms));
        selected.head.authority = None;
        let writes = [
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
                &evidence_id,
                &receipt.owner,
                &evidence,
                Write {
                    retained_until: Some(receipt.retained_until),
                    ..Default::default()
                },
            )?,
            store.prepare(
                &identifier("replica-abandonment-index-v1", &index.deployment),
                &index.deployment,
                &index,
                Write {
                    expected: (version > 0).then_some(version),
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
        ];
        match store.batch(&writes).await {
            Ok(_) => Ok(receipt),
            Err(error) => {
                if let Some((saved, _)) = store.read::<Receipt>(&audit_id, true).await? {
                    return if hash(&saved.request)? == hash(req)? {
                        Ok(saved)
                    } else {
                        Err(Failure::refuse(
                            "request_id_reuse",
                            serde_json::json!({"existing_binding": saved.request.binding, "target": saved.target}),
                        ))
                    };
                }
                if error == Error::Busy {
                    if let Err(changed) = self.select(&req.binding, false).await {
                        if changed.code() == "binding_version_mismatch" {
                            return Err(changed);
                        }
                    }
                }
                Err(error.into())
            }
        }
    }
}
