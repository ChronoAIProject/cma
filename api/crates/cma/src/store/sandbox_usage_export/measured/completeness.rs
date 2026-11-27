//! Publish measured completeness only from an exact closed lifecycle timeline.
use super::{
    freeze::{Manifest, MeasuredPublication},
    *,
};
use crate::store::sandbox_usage_export::completeness::MinuteOutcome;
use crate::{
    sandbox_usage_export::completeness_contract::{
        Basis, Contribution, ContributionKind, Coverage, Dimension, FactRef, Interval, Number,
        Position, Resource,
    },
    store::sandbox_events::TransactionEventError,
};
use layer2_core::{
    sandbox_metrics::{BindingKind, ProofStatus, Range},
    sandbox_usage_evidence::{MeasuredMetric, MeasuredQuality},
};
use mongodb::ClientSession;

fn coverage(metric: &MeasuredMetric) -> Coverage {
    match metric.quality {
        MeasuredQuality::Observed if metric.quantity.as_deref() == Some("0") => {
            Coverage::ObservedZero
        }
        MeasuredQuality::Observed => Coverage::Observed,
        MeasuredQuality::Partial => Coverage::Partial,
        MeasuredQuality::Missing => Coverage::Missing,
        MeasuredQuality::Estimated => Coverage::Estimated,
    }
}

fn resource(input: &MeasuredInput, dimension: Dimension) -> Option<Resource> {
    match dimension {
        Dimension::Cpu | Dimension::Memory => Some(Resource {
            id: input.resource.runtime_id.clone()?,
            generation: input.resource.runtime_generation?.to_string(),
        }),
        Dimension::Disk => Some(Resource {
            id: input.resource.home_id.clone()?,
            generation: input.resource.home_generation.clone()?,
        }),
    }
}

fn kind(dimension: Dimension) -> BindingKind {
    match dimension {
        Dimension::Cpu | Dimension::Memory => BindingKind::Compute,
        Dimension::Disk => BindingKind::Home,
    }
}

fn residual_ranges(physical: &[Range], existing: &[Range], bucket: Range) -> Vec<Range> {
    let mut endpoints = vec![bucket.from, bucket.to];
    for interval in physical.iter().chain(existing) {
        endpoints.push(interval.from);
        endpoints.push(interval.to);
    }
    endpoints.sort_unstable();
    endpoints.dedup();
    let mut result: Vec<Range> = vec![];
    for points in endpoints.windows(2) {
        let range = Range {
            from: points[0],
            to: points[1],
        };
        if range.from < bucket.from || range.to > bucket.to || range.from == range.to {
            continue;
        }
        let contains = |other: &Range| other.from <= range.from && other.to >= range.to;
        if physical.iter().any(contains) && !existing.iter().any(contains) {
            match result.last_mut() {
                Some(last) if last.to == range.from => last.to = range.to,
                _ => result.push(range),
            }
        }
    }
    result
}

impl CaptureStore {
    pub(super) async fn stage_measured_hold(
        &self,
        session: &mut ClientSession,
        stream: &MeasuredStream,
        publication: &MeasuredPublication,
    ) -> Result<(), TransactionEventError> {
        let input = &publication.manifest.input;
        let Some(mut previous) = self
            .retained_statement(
                session,
                &stream.source_stream,
                input.minute,
                Basis::Measured,
            )
            .await?
        else {
            return Ok(());
        };
        previous.producer_hold = publication.manifest.hold.clone();
        if previous.producer_hold.is_none() {
            return Ok(());
        }
        previous.measured_mapping_revision = Some(Number(
            u64::try_from(input.mapping_revision)
                .map_err(|_| TransactionEventError::Domain(corrupt()))?,
        ));
        let outcome = self
            .stage_statement(session, &stream.source_stream, input.minute, previous)
            .await?;
        self.record_outcome(
            session,
            &stream.source_stream,
            input.minute,
            Basis::Measured,
            outcome,
        )
        .await
    }

    pub(super) async fn stage_measured_completeness(
        &self,
        session: &mut ClientSession,
        stream: &MeasuredStream,
        manifest: &Manifest,
        head: &FactHead,
    ) -> Result<MinuteOutcome, TransactionEventError> {
        use TransactionEventError::Domain;
        let input = &manifest.input;
        if let Some(previous) = self
            .retained_statement(
                session,
                &stream.source_stream,
                input.minute,
                Basis::Measured,
            )
            .await?
        {
            if previous.producer_hold.is_some()
                && previous
                    .measured_mapping_revision
                    .is_some_and(|revision| revision.0 > input.mapping_revision as u64)
            {
                return Ok(MinuteOutcome::Retry("newer_mapping_hold"));
            }
        }
        let Some(mut statement) = self
            .retained_statement(
                session,
                &stream.source_stream,
                input.minute,
                Basis::Allocated,
            )
            .await?
        else {
            return Ok(MinuteOutcome::Retry("allocated_statement_pending"));
        };
        if statement.source_epoch != stream.source_epoch
            || statement.owner.subject != stream.owner
            || statement.sandbox_id != stream.sandbox_id
            || statement.scope.start_ms.0 != input.minute * 1000
            || statement.scope.end_ms.0 != (input.minute + 60) * 1000
        {
            return Ok(MinuteOutcome::Held("source_scope_mismatch"));
        }
        // The immutable source captures in the measured snapshot must be exactly
        // the allocated statement's timeline. A later capture cannot insert into
        // the already closed monotonic source boundary.
        if statement.timeline.len() != input.captures.len() {
            return Ok(MinuteOutcome::Retry("timeline_count_changed"));
        }
        for (slice, capture) in statement.timeline.iter().zip(&input.captures) {
            if slice.source_revision.0 != capture.source.revision
                || slice.source_digest
                    != hash(&serde_json::to_vec(capture).map_err(|_| Domain(corrupt()))?)
            {
                return Ok(MinuteOutcome::Retry("timeline_source_changed"));
            }
        }
        let Some(frozen) = manifest.frozen.as_ref() else {
            return Ok(MinuteOutcome::Retry("measured_manifest_missing"));
        };
        let fact = frozen.fact();
        let mut contributions = std::mem::take(&mut statement.contributions);
        let bucket = Range {
            from: input.minute,
            to: input.minute + 60,
        };
        for (index, dimension) in [Dimension::Cpu, Dimension::Memory, Dimension::Disk]
            .into_iter()
            .enumerate()
        {
            let metric = &fact.metrics[index];
            let binding_kind = kind(dimension);
            let resource = resource(input, dimension);
            let mut binding_revisions = input
                .bindings
                .iter()
                .filter(|binding| {
                    binding.kind == binding_kind
                        && binding.sandbox_id == stream.sandbox_id
                        && binding.owner_subject == stream.owner
                })
                .map(|binding| Number(binding.revision))
                .collect::<Vec<_>>();
            binding_revisions.sort_unstable();
            binding_revisions.dedup();
            let mut existing = vec![];
            for contribution in contributions
                .iter_mut()
                .filter(|contribution| contribution.dimension == dimension)
            {
                if resource.as_ref() != Some(&contribution.resource) {
                    return Ok(MinuteOutcome::Held("mixed_resource_incarnation"));
                }
                existing.push(Range {
                    from: contribution.interval.start_ms.0 / 1000,
                    to: contribution.interval.end_ms.0 / 1000,
                });
                contribution.binding_revisions = binding_revisions.clone();
                contribution.facts = vec![head.fact_id.clone()];
                contribution.coverage = coverage(metric);
            }
            let physical = input
                .binding_segments()
                .into_iter()
                .map(|(_, binding)| binding)
                .filter(|binding| {
                    binding.kind == binding_kind
                        && binding.sandbox_id == stream.sandbox_id
                        && binding.owner_subject == stream.owner
                        && binding.proof_status == ProofStatus::Verified
                })
                .map(|binding| binding.range(bucket.to).intersect(bucket))
                .filter(|range| range.seconds() > 0)
                .collect::<Vec<_>>();
            let residual = residual_ranges(&physical, &existing, bucket);
            let total = existing
                .iter()
                .chain(&residual)
                .map(|range| range.seconds())
                .sum::<u64>()
                .checked_mul(1000)
                .ok_or_else(|| Domain(corrupt()))?;
            if metric.expected_binding_ms != total.to_string() {
                return Ok(MinuteOutcome::Retry("measured_extent_mismatch"));
            }
            for range in residual {
                let Some(resource) = resource.clone() else {
                    return Ok(MinuteOutcome::Retry("residual_resource_missing"));
                };
                contributions.push(Contribution {
                    dimension,
                    resource,
                    interval: Interval {
                        start_ms: Number(range.from * 1000),
                        end_ms: Number(range.to * 1000),
                    },
                    kind: ContributionKind::Residual,
                    binding_revisions: binding_revisions.clone(),
                    facts: vec![head.fact_id.clone()],
                    coverage: coverage(metric),
                });
            }
        }
        statement.basis = Basis::Measured;
        statement.producer_hold = None;
        statement.measured_mapping_revision = Some(Number(
            u64::try_from(input.mapping_revision).map_err(|_| Domain(corrupt()))?,
        ));
        statement.positions.push(Position {
            basis: Basis::Measured,
            stream_id: stream.id.clone(),
            sequence: Number(stream.sequence),
            digest: stream
                .previous_digest
                .clone()
                .ok_or_else(|| Domain(corrupt()))?,
        });
        statement.facts = vec![FactRef {
            stream_id: stream.id.clone(),
            fact_id: head.fact_id.clone(),
            event_id: head.event_id.clone(),
            event_digest: head.digest.clone(),
            evidence_revision: Number(head.revision),
        }];
        statement.contributions = contributions;
        self.stage_statement(session, &stream.source_stream, input.minute, statement)
            .await
    }
}
