use super::*;
use crate::records::mongo::ReplicaKind;
use crate::replicas::diagnostics::{Class, ScanError, Stage};

pub(super) struct LaneScan {
    pub views: Vec<LaneView>,
    pub unreadable: Vec<Unreadable>,
    pub skipped: usize,
    pub truncated: bool,
    pub outstanding_lane_items: usize,
}

impl Operator {
    pub(super) async fn lane_views(&self) -> Result<LaneScan, Error> {
        self.scan_lanes(4096).await
    }

    pub(super) async fn scan_lanes(&self, limit: usize) -> Result<LaneScan, Error> {
        let mut scan = LaneScan {
            views: Vec::new(),
            unreadable: Vec::new(),
            skipped: 0,
            truncated: false,
            outstanding_lane_items: 0,
        };
        let mut cursor = None;
        let now = self
            .partitions
            .store
            .clock()
            .await
            .inspect_err(|&error| self.report_lane_scan(error, Stage::Clock))?
            .millis
            / 1000;
        loop {
            let rows = self
                .partitions
                .store
                .active_entries_in::<Head>(
                    ReplicaKind::Partition,
                    &self.partitions.membership.deployment,
                    cursor.as_deref(),
                    128,
                    true,
                )
                .await
                .inspect_err(|&error| self.report_lane_scan(error, Stage::HeadRead))?;
            let count = rows.len();
            for entry in rows {
                if scan.views.len() + scan.unreadable.len() == limit {
                    scan.truncated = true;
                    return Ok(scan);
                }
                let id = entry.id;
                cursor = Some(id.clone());
                let head_version = entry.version;
                let head = match entry.value {
                    Ok(head) => head,
                    Err(error) => {
                        self.lane_unreadable(
                            &mut scan,
                            id.clone(),
                            head_version,
                            None,
                            ScanError::new(error, Stage::HeadDecode, Some(&id)),
                        );
                        continue;
                    }
                };
                scan.outstanding_lane_items += head.queue.len();
                if let Err(error) =
                    head.validate(&id, &self.partitions.membership.deployment, &head.owner)
                {
                    self.lane_unreadable(
                        &mut scan,
                        id.clone(),
                        head_version,
                        head.queue.front().cloned(),
                        ScanError::new(error, Stage::HeadValidate, Some(&id)),
                    );
                    continue;
                }
                let Some(work) = head.queue.front() else {
                    self.lane_unreadable(
                        &mut scan,
                        id.clone(),
                        head_version,
                        None,
                        ScanError::new(Class::Corrupt, Stage::Front, Some(&id)),
                    );
                    continue;
                };
                let loaded = match crate::replicas::head_evidence::load(
                    &self.partitions,
                    &self.plans,
                    &id,
                    &head,
                    false,
                )
                .await
                {
                    Ok(Some(loaded)) => loaded,
                    Ok(None) => {
                        self.lane_unreadable(
                            &mut scan,
                            id.clone(),
                            head_version,
                            Some(work.clone()),
                            ScanError::new(Class::Corrupt, Stage::ItemCheck, Some(&id)),
                        );
                        continue;
                    }
                    Err(error) => {
                        self.lane_unreadable(
                            &mut scan,
                            id.clone(),
                            head_version,
                            Some(work.clone()),
                            error,
                        );
                        continue;
                    }
                };
                let item = loaded.item;
                let plan = loaded.plan;
                let target = &plan.targets[loaded.target_index];
                if super::super::delayed_lane::id(
                    &self.partitions.membership.deployment,
                    &plan,
                    target,
                ) != id
                {
                    self.lane_unreadable(
                        &mut scan,
                        id.clone(),
                        head_version,
                        Some(work.clone()),
                        ScanError::new(Class::Mismatch, Stage::PlanCheck, Some(&id)),
                    );
                    continue;
                }
                let admitted = match self
                    .partitions
                    .store
                    .read::<super::super::delayed_lane::AdmissionTime>(
                        &super::super::delayed_lane::time_id(work),
                        true,
                    )
                    .await
                {
                    Ok(Some((admitted, _))) => admitted,
                    Ok(None) => {
                        self.lane_unreadable(
                            &mut scan,
                            id.clone(),
                            head_version,
                            Some(work.clone()),
                            ScanError::new(Class::MissingItem, Stage::ItemRead, Some(&id)),
                        );
                        continue;
                    }
                    Err(error) => {
                        self.lane_unreadable(
                            &mut scan,
                            id.clone(),
                            head_version,
                            Some(work.clone()),
                            ScanError::new(error, Stage::ItemRead, Some(&id)),
                        );
                        continue;
                    }
                };
                if admitted.kind != "delayed_lane_admission_v1"
                    || admitted.target != target.id
                    || admitted.work != *work
                    || admitted.admitted_at <= 0
                    || admitted.admitted_at > now
                {
                    self.lane_unreadable(
                        &mut scan,
                        id.clone(),
                        head_version,
                        Some(work.clone()),
                        ScanError::new(Class::Mismatch, Stage::ItemCheck, Some(&id)),
                    );
                    continue;
                }
                let retained_until = match utc(item.retained_until) {
                    Ok(retained_until) => retained_until,
                    Err(error) => {
                        self.lane_unreadable(
                            &mut scan,
                            id.clone(),
                            head_version,
                            Some(work.clone()),
                            ScanError::new(error, Stage::Evidence, Some(&id)),
                        );
                        continue;
                    }
                };
                scan.views.push(LaneView {
                    id,
                    target: item.target,
                    depth: head.queue.len(),
                    oldest_age_seconds: (now - admitted.admitted_at).max(0),
                    lease_until_ms: head.authority.map(|authority| authority.until_ms),
                    retained_until,
                });
            }
            if count < 128 {
                return Ok(scan);
            }
        }
    }

    fn report_lane_scan(&self, error: Error, stage: Stage) {
        self.partitions
            .isolation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .report(&ScanError::new(error, stage, None));
    }

    fn lane_unreadable(
        &self,
        scan: &mut LaneScan,
        partition: String,
        head_version: Option<u64>,
        head_work: Option<String>,
        error: ScanError,
    ) {
        self.partitions
            .isolation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .report(&error);
        scan.skipped += 1;
        scan.unreadable.push(Unreadable {
            partition,
            partition_hash: error.partition_hash,
            head_version,
            head_work,
            class: error.class,
            stage: error.stage,
            age_seconds: error.age_seconds,
            custody: Some("delayed_lane".into()),
        });
    }
}
