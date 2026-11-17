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
        for ephemeral in [false, true] {
            let mut cursor = None;
            loop {
                let rows = self
                    .partitions
                    .store
                    .active_entries_in::<Head>(
                        ReplicaKind::Partition,
                        &self.partitions.membership.deployment,
                        cursor.as_deref(),
                        128,
                        ephemeral,
                    )
                    .await
                    .inspect_err(|&e| self.report_scan(e, Stage::HeadRead))?;
                let count = rows.len();
                let now = self
                    .partitions
                    .store
                    .clock()
                    .await
                    .inspect_err(|&e| self.report_scan(e, Stage::Clock))?
                    .millis;
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
                    if head.authority.as_ref().is_some_and(|a| a.until_ms > now) {
                        output.skipped += 1;
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
                        Ok(selected) if selected.candidate.lease_live => output.skipped += 1,
                        Ok(selected) if selected.candidate.eligible => {
                            output.candidates.push(selected.candidate)
                        }
                        Ok(_) => {}
                        Err(error) if error.transient_head() => output.skipped += 1,
                        Err(error) => {
                            let diagnostic = match error {
                                Failure::Head(error) => error,
                                other => {
                                    ScanError::new(other.storage(), Stage::Evidence, Some(&id))
                                }
                            };
                            self.unreadable(
                                &mut output,
                                id,
                                version,
                                Some(work.clone()),
                                diagnostic,
                            );
                        }
                    }
                }
                if count < 128 {
                    break;
                }
            }
        }
        Ok(output)
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
            custody: None,
        });
    }
    pub async fn observe_heads(&self) -> Result<(), Error> {
        let scan = self.head_candidates().await?;
        let lane_scan = self.lane_views().await?;
        self.observe_scans(scan, lane_scan)
    }
    pub(super) fn observe_scans(
        &self,
        scan: HeadScan,
        lane_scan: super::lanes::LaneScan,
    ) -> Result<(), Error> {
        let lanes = lane_scan.views;
        let truncated = scan.truncated || lane_scan.truncated;
        metrics::gauge!("cmaeg_replica_delayed_lane_depth")
            .set(lanes.iter().map(|lane| lane.depth).sum::<usize>() as f64);
        metrics::gauge!("cmaeg_replica_delayed_lane_oldest_age_seconds").set(
            lanes
                .iter()
                .map(|lane| lane.oldest_age_seconds)
                .max()
                .unwrap_or(0) as f64,
        );
        metrics::gauge!("cmaeg_replica_uncertainty_observation_skipped")
            .set((scan.skipped + lane_scan.skipped) as f64);
        metrics::gauge!("cmaeg_replica_uncertainty_observation_truncated")
            .set(u8::from(truncated) as f64);
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
        if !truncated {
            seen.retain(|work| scan.candidates.iter().any(|c| &c.binding.head_work == work));
        }
        Ok(())
    }
}
