use super::*;
use crate::records::mongo::ReplicaKind;
use crate::replicas::diagnostics::{Class, ScanError, Stage};

pub(super) struct HeadScan {
    pub candidates: Vec<Candidate>,
    pub unreadable: Vec<Unreadable>,
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
            unreadable: Vec::new(),
            skipped: 0,
            truncated: false,
        };
        let mut scanned = 0usize;
        let mut cursor = None;
        loop {
            let rows = self
                .partitions
                .store
                .active_entries::<Head>(
                    ReplicaKind::Partition,
                    &self.partitions.membership.deployment,
                    cursor.as_deref(),
                    128,
                )
                .await
                .inspect_err(|&e| self.report_scan(e, Stage::HeadRead))?;
            let count = rows.len();
            for entry in rows {
                let id = entry.id;
                let version = entry.version;
                if scanned == limit {
                    output.truncated = true;
                    return Ok(output);
                }
                scanned += 1;
                cursor = Some(id.clone());
                let head = match entry.value {
                    Ok(head) => head,
                    Err(error) => {
                        self.unreadable(
                            &mut output,
                            id.clone(),
                            version,
                            None,
                            ScanError::new(error, Stage::HeadDecode, Some(&id)),
                        );
                        continue;
                    }
                };
                if let Err(error) =
                    head.validate(&id, &self.partitions.membership.deployment, &head.owner)
                {
                    self.unreadable(
                        &mut output,
                        id.clone(),
                        version,
                        head.queue.front().cloned(),
                        ScanError::new(error, Stage::HeadValidate, Some(&id)),
                    );
                    continue;
                }
                let Some(work) = head.queue.front() else {
                    self.unreadable(
                        &mut output,
                        id.clone(),
                        version,
                        None,
                        ScanError::new(Class::Corrupt, Stage::Front, Some(&id)),
                    );
                    continue;
                };
                match self
                    .select(
                        &Binding {
                            partition: id.clone(),
                            head_work: work.clone(),
                            head_version: version.ok_or(Error::Corrupt)?,
                        },
                        false,
                    )
                    .await
                {
                    Ok(Selected::Missing(selected)) => {
                        if selected.candidate.eligible {
                            output.candidates.push(selected.candidate);
                        } else {
                            self.unreadable(
                                &mut output,
                                id.clone(),
                                version,
                                Some(work.clone()),
                                ScanError::new(Class::MissingItem, Stage::ItemRead, Some(&id)),
                            );
                        }
                    }
                    Ok(Selected::Normal(selected)) if selected.candidate.lease_live => {
                        output.skipped += 1
                    }
                    Ok(Selected::Normal(selected)) if selected.candidate.eligible => {
                        output.candidates.push(selected.candidate)
                    }
                    Ok(_) => {}
                    Err(error) if error.transient_head() => output.skipped += 1,
                    Err(error) => {
                        let diagnostic = match error {
                            Failure::Head(error) => error,
                            Failure::Refusal {
                                code: "missing_item_mismatch",
                                ..
                            } => ScanError::new(Class::Mismatch, Stage::ItemCheck, Some(&id)),
                            other => ScanError::new(other.storage(), Stage::Evidence, Some(&id)),
                        };
                        self.unreadable(&mut output, id, version, Some(work.clone()), diagnostic);
                    }
                }
            }
            if count < 128 {
                return Ok(output);
            }
        }
    }
    fn report_scan(&self, error: Error, stage: Stage) {
        self.partitions
            .isolation
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .report(&ScanError::new(error, stage, None));
    }
    fn unreadable(
        &self,
        scan: &mut HeadScan,
        partition: String,
        head_version: Option<u64>,
        head_work: Option<String>,
        error: ScanError,
    ) {
        self.partitions
            .isolation
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .report(&error);
        scan.unreadable.push(Unreadable {
            partition,
            partition_hash: error.partition_hash,
            head_version,
            head_work,
            class: error.class,
            stage: error.stage,
            age_seconds: error.age_seconds,
        });
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
                tracing::warn!(partition_hash=%c.partition_hash, state=%c.state, age_seconds=c.age_seconds, "partition head retained uncertain; operator inspection required");
            }
        }
        if !scan.truncated {
            seen.retain(|work| scan.candidates.iter().any(|c| &c.binding.head_work == work));
        }
        Ok(())
    }
}
