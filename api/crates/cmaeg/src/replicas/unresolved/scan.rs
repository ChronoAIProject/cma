use super::*;
use crate::records::mongo::ReplicaKind;

pub(super) struct HeadScan {
    pub candidates: Vec<Candidate>,
    pub skipped: usize,
    pub truncated: bool,
}
impl Operator {
    pub(super) async fn head_candidates(&self) -> Result<HeadScan, Error> {
        self.scan_heads(4096).await
    }
    pub(super) async fn scan_heads(&self, limit: usize) -> Result<HeadScan, Error> {
        let mut output = HeadScan {
            candidates: Vec::new(),
            skipped: 0,
            truncated: false,
        };
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
            let count = rows.len();
            let now = self.partitions.store.clock().await?.millis;
            for (id, head, version) in rows {
                if scanned == limit {
                    output.truncated = true;
                    return Ok(output);
                }
                scanned += 1;
                cursor = Some(id.clone());
                head.validate(&id, &self.partitions.membership.deployment, &head.owner)?;
                if head.authority.as_ref().is_some_and(|a| a.until_ms > now) {
                    output.skipped += 1;
                    continue;
                }
                let Some(work) = head.queue.front() else {
                    output.skipped += 1;
                    continue;
                };
                match self
                    .select(
                        &Binding {
                            partition: id,
                            head_work: work.clone(),
                            head_version: version,
                        },
                        false,
                    )
                    .await
                {
                    Ok(selected) if selected.candidate.lease_live => output.skipped += 1,
                    Ok(selected) if selected.candidate.eligible => {
                        output.candidates.push(selected.candidate)
                    }
                    Ok(_) => {}
                    Err(error) if error.transient_head() => output.skipped += 1,
                    Err(error) => return Err(error.storage()),
                }
            }
            if count < 128 {
                return Ok(output);
            }
        }
    }
    pub async fn observe_heads(&self) -> Result<(), Error> {
        let scan = self.head_candidates().await?;
        metrics::gauge!("cmaeg_replica_uncertainty_observation_skipped").set(scan.skipped as f64);
        metrics::gauge!("cmaeg_replica_uncertainty_observation_truncated")
            .set(u8::from(scan.truncated) as f64);
        metrics::gauge!("cmaeg_replica_retained_uncertain_heads").set(scan.candidates.len() as f64);
        metrics::gauge!("cmaeg_replica_retained_uncertain_oldest_age_seconds").set(
            scan.candidates
                .iter()
                .map(|c| c.age_seconds)
                .max()
                .unwrap_or(0) as f64,
        );
        let mut seen = self.seen.lock().map_err(|_| Error::Lost)?;
        for c in &scan.candidates {
            if seen.insert(c.binding.head_work.clone()) {
                tracing::warn!(partition=%c.binding.partition, work=%c.binding.head_work, state=%c.state, age_seconds=c.age_seconds, "partition head retained uncertain; operator inspection required");
            }
        }
        if !scan.truncated {
            seen.retain(|work| scan.candidates.iter().any(|c| &c.binding.head_work == work));
        }
        Ok(())
    }
}
