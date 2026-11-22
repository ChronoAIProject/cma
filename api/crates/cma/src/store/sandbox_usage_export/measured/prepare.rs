use super::*;
use crate::store::{sandbox_metric_bindings, usage_reports};
use layer2_core::sandbox_metrics::{Binding, BindingKind, ProofStatus, Range};
use layer2_core::sandbox_usage_evidence::ExpectedMinute;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MeasuredInput {
    pub(super) stream: MeasuredStream,
    pub(super) minute: u64,
    pub(super) replay: bool,
    pub(super) mapping_revision: i64,
    pub(super) captures: Vec<Captured>,
    pub(super) bindings: Vec<Binding>,
    pub(super) bridges: Vec<BindingProof>,
    pub(super) expected: ExpectedMinute,
    pub(super) resource: ResourceSnapshot,
    pub(super) hold: Option<String>,
    pub(super) predecessor: Option<FactHead>,
    #[serde(skip)]
    pub(super) previous_evidence: Option<super::replay::PreviousEvidence>,
}

impl MeasuredInput {
    pub(crate) fn range(&self) -> Range {
        Range {
            from: self.minute.saturating_sub(120),
            to: self.minute + 180,
        }
    }

    pub(crate) fn bindings(&self) -> Vec<Binding> {
        self.binding_segments()
            .into_iter()
            .map(|(_, binding)| binding)
            .collect()
    }

    pub(super) fn binding_segments(&self) -> Vec<(String, Binding)> {
        let mut result = Vec::new();
        for binding in &self.bindings {
            if binding.sandbox_id != self.stream.sandbox_id
                || binding.owner_subject != self.stream.owner
            {
                result.push((binding.id.clone(), binding.clone()));
                continue;
            }
            let proofs: Vec<_> = self
                .bridges
                .iter()
                .filter(|p| p.binding.id == binding.id)
                .collect();
            if proofs.is_empty() {
                let mut binding = binding.clone();
                binding.proof_status = ProofStatus::Unattributed;
                result.push((binding.id.clone(), binding));
            } else {
                for proof in proofs {
                    let original_id = binding.id.clone();
                    let mut binding = binding.clone();
                    binding.id = format!("{}/{:016x}", binding.id, proof.verified_from);
                    binding.valid_from = binding.valid_from.max(proof.verified_from);
                    binding.valid_until = Some(
                        binding
                            .range(u64::MAX)
                            .to
                            .min(proof.verified_until)
                            .max(binding.valid_from),
                    );
                    result.push((original_id, binding));
                }
            }
        }
        result
    }
}

impl CaptureStore {
    pub(crate) async fn prepare_measured(
        &self,
        lease: &MeasuredLease,
    ) -> Result<Option<MeasuredInput>, SandboxStoreError> {
        let stream = &lease.stream;
        let cutoff = lease.now.saturating_sub(120) / 60 * 60;
        let floor = stream
            .capture_start
            .max(lease.now.saturating_sub(30 * 86400) / 60 * 60);
        let replay = (stream.replay_turn || stream.next_minute >= cutoff)
            && stream.replay_minute.max(floor) < stream.next_minute;
        let minute = if replay {
            stream.replay_minute.max(floor)
        } else {
            stream.next_minute
        };
        if minute >= cutoff || (replay && minute >= stream.next_minute) {
            return Ok(None);
        }
        self.read_measured_input(stream, minute, replay)
            .await
            .map(Some)
    }

    async fn read_measured_input(
        &self,
        stream: &MeasuredStream,
        minute: u64,
        replay: bool,
    ) -> Result<MeasuredInput, SandboxStoreError> {
        let mut session = self
            .client
            .start_session()
            .snapshot(true)
            .await
            .map_err(unavailable)?;
        let control = self
            .collection(usage_reports::CONTROL)
            .find_one(doc! {"_id":"observer"})
            .session(&mut session)
            .await
            .map_err(unavailable)?
            .ok_or_else(corrupt)?;
        let revision = control.get_i64("revision").map_err(unavailable)?;
        if revision < 0 {
            return Err(corrupt());
        }
        let source = Stream::read(
            &self
                .collection(STREAMS)
                .find_one(doc! {"_id":&stream.source_stream})
                .session(&mut session)
                .await
                .map_err(unavailable)?
                .ok_or_else(corrupt)?,
        )?;
        if source.source_epoch != stream.source_epoch
            || source.owner != stream.owner
            || source.namespace != stream.namespace
        {
            return Err(corrupt());
        }
        let mut cursor = self.collection(CAPTURES).find(doc! {"stream_id":&stream.source_stream,
            "record.source.started_at":{"$lt":number(minute + 60)?},"record.source.ended_at":{"$gt":number(minute)?}})
            .sort(doc! {"revision":1}).limit(65).max_time(std::time::Duration::from_secs(2))
            .session(&mut session).await.map_err(unavailable)?;
        let mut captures = Vec::new();
        while let Some(row) = cursor
            .next(&mut session)
            .await
            .transpose()
            .map_err(unavailable)?
        {
            captures.push(super::super::project::read_capture(&row)?);
        }
        if captures.len() > 64 {
            return Err(unavailable("minute source bound"));
        }
        let mut expected = ExpectedMinute {
            sandbox_id: stream.sandbox_id.clone(),
            owner: stream.owner.clone(),
            bucket_start: minute,
            compute: vec![],
            home: vec![],
        };
        let mut resource = ResourceSnapshot::default();
        let mut hold = None;
        let mut covered_until = minute;
        for capture in &captures {
            let event = &capture.source;
            if capture.stream_id != source.id
                || event.owner.as_str() != stream.owner
                || event.sandbox_id.as_str() != stream.sandbox_id
            {
                return Err(corrupt());
            }
            let range = Range {
                from: event.started_at.max(minute),
                to: event.ended_at.min(minute + 60),
            };
            if capture.source_gap || range.from != covered_until {
                hold = Some("lifecycle_gap".into());
            }
            covered_until = range.to;
            if event.allocation.running {
                expected.compute.push(range);
                merge_runtime(&mut resource, &capture.resource, &mut hold);
            }
            if event.allocation.disk_gib > 0 {
                expected.home.push(range);
                merge_home(&mut resource, &capture.resource, &mut hold);
            }
        }
        if covered_until != minute + 60 {
            hold = Some("lifecycle_incomplete".into());
        }
        let time = doc! {"valid_from":{"$lt":number(minute+60)?},"$or":[{"valid_until":null},{"valid_until":{"$gt":number(minute)?}}]};
        let mut filter = time.clone();
        filter.insert("sandbox_id", &stream.sandbox_id);
        let mut bindings = self.measured_bindings(filter, &mut session).await?;
        let physical: Vec<_> = bindings.values().map(Binding::physical_key).collect();
        if !physical.is_empty() {
            let mut filter = time;
            filter.insert("physical_key", doc! {"$in":physical});
            bindings.extend(self.measured_bindings(filter, &mut session).await?);
        }
        if bindings.len() > 64 {
            return Err(unavailable("minute binding bound"));
        }
        let mut bridges = Vec::new();
        for binding in bindings
            .values()
            .filter(|b| b.sandbox_id == stream.sandbox_id && b.owner_subject == stream.owner)
        {
            let mut filter = keys::page(&keys::bridge(&binding.id), None)?;
            filter.insert("record.verified_from", doc! {"$lt":number(minute+60)?});
            filter.insert("record.verified_until", doc! {"$gt":number(minute)?});
            let mut cursor = self
                .collection(INVENTORY)
                .find(filter)
                .limit(65)
                .max_time(std::time::Duration::from_secs(2))
                .session(&mut session)
                .await
                .map_err(unavailable)?;
            while let Some(row) = cursor
                .next(&mut session)
                .await
                .transpose()
                .map_err(unavailable)?
            {
                if bridges.len() >= 64 {
                    return Err(unavailable("bridge interval bound"));
                }
                let bridge: BindingProof = decode(&row)?;
                let matches = match binding.kind {
                    BindingKind::Compute => {
                        bridge.resource.runtime_id == resource.runtime_id
                            && bridge.resource.runtime_generation == resource.runtime_generation
                    }
                    BindingKind::Home => {
                        bridge.resource.home_id == resource.home_id
                            && bridge.resource.home_generation == resource.home_generation
                    }
                };
                // Retained lifecycle snapshots also authorize observed residual
                // physical use after compute allocation has stopped.
                let residual = captures.iter().any(|c| match binding.kind {
                    BindingKind::Compute => {
                        c.resource.runtime_id == bridge.resource.runtime_id
                            && c.resource.runtime_generation == bridge.resource.runtime_generation
                    }
                    BindingKind::Home => {
                        c.resource.home_id == bridge.resource.home_id
                            && c.resource.home_generation == bridge.resource.home_generation
                    }
                });
                if !matches && residual {
                    match binding.kind {
                        BindingKind::Compute => {
                            merge_runtime(&mut resource, &bridge.resource, &mut hold)
                        }
                        BindingKind::Home => merge_home(&mut resource, &bridge.resource, &mut hold),
                    }
                }
                if (matches || residual)
                    && bridge.binding.id == binding.id
                    && bridge.binding.physical_key() == binding.physical_key()
                    && bridge.binding.sandbox_id == binding.sandbox_id
                    && bridge.binding.owner_subject == binding.owner_subject
                {
                    bridges.push(bridge);
                } else {
                    hold = Some("resource_attribution_conflict".into());
                }
            }
        }
        let predecessor = self
            .collection(INVENTORY)
            .find_one(doc! {"_id":keys::id(&keys::head(&stream.id,minute))})
            .session(&mut session)
            .await
            .map_err(unavailable)?
            .as_ref()
            .map(decode)
            .transpose()?;
        let previous_evidence = match &predecessor {
            Some(head) => Some(
                self.previous_measured_evidence(stream, minute, head, &mut session)
                    .await?,
            ),
            None => None,
        };
        Ok(MeasuredInput {
            stream: stream.clone(),
            minute,
            replay,
            mapping_revision: revision,
            captures,
            bindings: bindings.into_values().collect(),
            bridges,
            expected,
            resource,
            hold,
            predecessor,
            previous_evidence,
        })
    }

    async fn measured_bindings(
        &self,
        filter: Document,
        session: &mut ClientSession,
    ) -> Result<BTreeMap<String, Binding>, SandboxStoreError> {
        let mut cursor = self
            .collection(sandbox_metric_bindings::COLLECTION)
            .find(filter)
            .limit(65)
            .max_time(std::time::Duration::from_secs(2))
            .session(&mut *session)
            .await
            .map_err(unavailable)?;
        let mut result = BTreeMap::new();
        while let Some(row) = cursor
            .next(&mut *session)
            .await
            .transpose()
            .map_err(unavailable)?
        {
            let binding: Binding = decode(&row)?;
            if row.get_str("_id").ok() != Some(&binding.id)
                || row.get_i64("revision").ok() != Some(number(binding.revision)?)
                || row.get_str("physical_key").ok() != Some(binding.physical_key().as_str())
            {
                return Err(corrupt());
            }
            result.insert(binding.id.clone(), binding);
        }
        if result.len() > 64 {
            return Err(unavailable("minute binding bound"));
        }
        Ok(result)
    }
}

fn merge_runtime(
    target: &mut ResourceSnapshot,
    source: &ResourceSnapshot,
    hold: &mut Option<String>,
) {
    if source.runtime_id.is_none() || source.runtime_generation.is_none() {
        *hold = Some("logical_runtime_missing".into());
    }
    if target.runtime_id.is_some()
        && (target.runtime_id != source.runtime_id
            || target.runtime_generation != source.runtime_generation)
    {
        *hold = Some("mixed_runtime_incarnations".into());
    } else {
        target.runtime_id = source.runtime_id.clone();
        target.runtime_generation = source.runtime_generation;
    }
}
fn merge_home(target: &mut ResourceSnapshot, source: &ResourceSnapshot, hold: &mut Option<String>) {
    if source.home_id.is_none() || source.home_generation.is_none() {
        *hold = Some("logical_home_missing".into());
    }
    if target.home_id.is_some()
        && (target.home_id != source.home_id || target.home_generation != source.home_generation)
    {
        *hold = Some("mixed_home_incarnations".into());
    } else {
        target.home_id = source.home_id.clone();
        target.home_generation = source.home_generation.clone();
    }
}
