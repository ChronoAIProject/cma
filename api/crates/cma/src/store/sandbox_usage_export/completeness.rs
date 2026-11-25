//! Stage declared minute completeness with the exact lifecycle projection transaction.
use super::{
    records::{hash, number, Captured, Stream},
    *,
};
use crate::{
    sandbox_usage_export::completeness_contract::{
        Basis, Boundary, Contribution, ContributionKind, Coverage, Dimension, FactRef, Interval,
        LifecycleSlice, Number, Owner, Position, Resource, Statement, MAX_ITEMS,
    },
    store::sandbox_events::TransactionEventError,
};
use mongodb::{
    bson::{spec::BinarySubtype, Binary},
    ClientSession,
};
use serde_json::Value;

mod backfill;
mod delivery;
mod outcome;
pub(crate) use delivery::{CompletenessAttempt, CompletenessReceipt, CompletenessResult};

fn key(namespace: &str, stream: &str, minute: u64, basis: Basis) -> Document {
    doc! {"usage_completeness_head_v1":format!("{}/{}/{minute:016x}/{basis:?}", hash(namespace.as_bytes()), stream)}
}

fn outbox_id(namespace: &str, id: &str) -> Document {
    doc! {"usage_completeness_outbox_v1":format!("{}/{id}", hash(namespace.as_bytes()))}
}

fn millis(seconds: u64) -> Result<Number, SandboxStoreError> {
    seconds
        .checked_mul(1000)
        .filter(|value| *value <= i64::MAX as u64)
        .map(Number)
        .ok_or_else(corrupt)
}

fn event_fact(event: &Value, minute: Number) -> Result<&Value, SandboxStoreError> {
    let minute = minute.0.to_string();
    let facts = event["facts"].as_array().ok_or_else(corrupt)?;
    facts
        .iter()
        .find(|fact| fact["bucket_start_ms"] == minute)
        .ok_or_else(corrupt)
}

fn field<'a>(value: &'a Value, name: &str) -> Result<&'a str, SandboxStoreError> {
    value[name].as_str().ok_or_else(corrupt)
}

fn resource(capture: &Captured, dimension: Dimension) -> Option<Resource> {
    match dimension {
        Dimension::Cpu | Dimension::Memory if capture.source.allocation.running => Some(Resource {
            id: capture.resource.runtime_id.clone()?,
            generation: capture.resource.runtime_generation?.to_string(),
        }),
        Dimension::Disk if capture.source.allocation.disk_gib > 0 => Some(Resource {
            id: capture.resource.home_id.clone()?,
            generation: capture.resource.home_generation.clone()?,
        }),
        _ => None,
    }
}

fn fact_reference(event: &FrozenUsageEvent, fact: &Value) -> Result<FactRef, SandboxStoreError> {
    Ok(FactRef {
        stream_id: event.stream_id.clone(),
        fact_id: field(fact, "fact_id")?.into(),
        event_id: event.event_id.clone(),
        event_digest: event.digest.clone(),
        evidence_revision: Number(
            field(fact, "evidence_revision")?
                .parse()
                .map_err(|_| corrupt())?,
        ),
    })
}

fn coverage(
    fact: &Value,
    dimension: Dimension,
    basis: Basis,
) -> Result<Coverage, SandboxStoreError> {
    let (dimension_name, allocated, measured) = match dimension {
        Dimension::Cpu => (
            "cpu",
            "sandbox_cpu_millis_seconds",
            "sandbox_cpu_core_nanoseconds_v1",
        ),
        Dimension::Memory => (
            "memory",
            "sandbox_memory_mib_seconds",
            "sandbox_memory_working_set_byte_milliseconds_v1",
        ),
        Dimension::Disk => (
            "disk",
            "sandbox_disk_gib_seconds",
            "sandbox_disk_used_byte_milliseconds_v1",
        ),
    };
    let quantity = &fact["quantities"][match basis {
        Basis::Allocated => allocated,
        Basis::Measured => measured,
    }];
    match field(&fact["coverage"][dimension_name], "quality")? {
        "observed" if quantity.as_str() == Some("0") => Ok(Coverage::ObservedZero),
        "observed" if quantity.is_string() => Ok(Coverage::Observed),
        "partial" => Ok(Coverage::Partial),
        "missing" => Ok(Coverage::Missing),
        "estimated" => Ok(Coverage::Estimated),
        _ => Err(corrupt()),
    }
}

enum AllocatedBuild {
    Ready(Box<Statement>),
    Retry(&'static str),
    Held(&'static str),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MinuteOutcome {
    Staged,
    Retry(&'static str),
    Held(&'static str),
}

impl CaptureStore {
    pub(crate) fn completeness_enabled(&self) -> bool {
        self.config.completeness_enabled
    }

    async fn allocated_statement(
        &self,
        session: &mut ClientSession,
        stream: &Stream,
        minute: u64,
        scope_start: u64,
        scope_end: u64,
    ) -> Result<AllocatedBuild, TransactionEventError> {
        use TransactionEventError::{Domain, Driver};
        let revision = stream.last_revision;
        let projected_through = stream.last_ended_at;
        if projected_through < scope_end {
            return Ok(AllocatedBuild::Retry("projection_incomplete"));
        }
        let mut rows = self.collection(CAPTURES).find(doc! {"stream_id":&stream.id,
            "record.source.started_at":{"$lt":number(scope_end).map_err(Domain)?},"record.source.ended_at":{"$gt":number(scope_start).map_err(Domain)?},
            "revision":{"$lte":number(revision).map_err(Domain)?}}).sort(doc! {"revision":1}).limit(MAX_ITEMS as i64+1).session(&mut *session).await.map_err(Driver)?;
        let mut timeline = Vec::new();
        let mut facts = Vec::new();
        let mut contributions = Vec::new();
        let mut cursor = scope_start;
        while let Some(row) = rows.next(&mut *session).await.transpose().map_err(Driver)? {
            if timeline.len() >= MAX_ITEMS {
                return Ok(AllocatedBuild::Held("capture_item_bound"));
            }
            let capture = super::project::read_capture(&row).map_err(Domain)?;
            let source = &capture.source;
            let start = source.started_at.max(scope_start);
            let end = source.ended_at.min(scope_end);
            if capture.source_gap
                || capture.stream_id != stream.id
                || source.owner.as_str() != stream.owner
                || source.sandbox_id.as_str() != stream.sandbox_id
                || start != cursor
                || end <= start
                || row.get_i64("next_second").map_err(|_| Domain(corrupt()))?
                    < number(end).map_err(Domain)?
            {
                return Ok(AllocatedBuild::Retry(
                    "lifecycle_gap_or_projection_incomplete",
                ));
            }
            let part = (minute / 60 - source.started_at / 60)
                / layer2_core::sandbox_usage_evidence::MAX_FACTS as u64;
            let Some(event) = self.collection(OUTBOX).find_one(doc! {"stream_id":&stream.id,"source_id":source.transaction_id(),"part":number(part).map_err(Domain)?})
                .session(&mut *session).await.map_err(Driver)? else {
                    return Ok(AllocatedBuild::Retry("projected_event_missing"));
                };
            let event = FrozenUsageEvent::read(&event).map_err(Domain)?;
            let value: Value =
                serde_json::from_slice(&event.payload).map_err(|_| Domain(corrupt()))?;
            let interval = Interval {
                start_ms: millis(start).map_err(Domain)?,
                end_ms: millis(end).map_err(Domain)?,
            };
            let fact = event_fact(&value, millis(minute).map_err(Domain)?).map_err(Domain)?;
            if field(fact, "start_ms").map_err(Domain)? != interval.start_ms.0.to_string()
                || field(fact, "end_ms").map_err(Domain)? != interval.end_ms.0.to_string()
            {
                return Ok(AllocatedBuild::Retry("lifecycle_fact_extent"));
            }
            let reference = fact_reference(&event, fact).map_err(Domain)?;
            let compute = resource(&capture, Dimension::Cpu);
            let home = resource(&capture, Dimension::Disk);
            if (source.allocation.running && compute.is_none())
                || (source.allocation.disk_gib > 0 && home.is_none())
            {
                return Ok(AllocatedBuild::Retry("resource_identity_missing"));
            }
            for dimension in [Dimension::Cpu, Dimension::Memory, Dimension::Disk] {
                if let Some(resource) = resource(&capture, dimension) {
                    contributions.push(Contribution {
                        dimension,
                        resource,
                        interval: interval.clone(),
                        kind: ContributionKind::Lifecycle,
                        binding_revisions: vec![],
                        facts: vec![reference.fact_id.clone()],
                        coverage: coverage(fact, dimension, Basis::Allocated).map_err(Domain)?,
                    });
                }
            }
            let source_digest = row
                .get_str("source_digest")
                .map_err(|_| Domain(corrupt()))?
                .to_owned();
            timeline.push(LifecycleSlice {
                interval,
                source_revision: Number(source.revision),
                source_digest,
                compute,
                home,
            });
            facts.push(reference);
            cursor = end;
        }
        if cursor != scope_end {
            return Ok(AllocatedBuild::Retry("lifecycle_extent_incomplete"));
        }
        facts.sort_by(|a, b| a.fact_id.cmp(&b.fact_id));
        let statement = Statement {
            schema_version: 1,
            statement_id: uuid::Uuid::new_v4().to_string(),
            revision: Number(1),
            previous_statement_digest: None,
            producer_namespace: stream.namespace.clone(),
            sandbox_id: stream.sandbox_id.clone(),
            source_epoch: stream.source_epoch.clone(),
            owner: Owner {
                issuer: stream.owner_issuer.clone(),
                subject: stream.owner.clone(),
                kind: "unresolved".into(),
            },
            basis: Basis::Allocated,
            minute_start_ms: millis(minute).map_err(Domain)?,
            scope: Interval {
                start_ms: millis(scope_start).map_err(Domain)?,
                end_ms: millis(scope_end).map_err(Domain)?,
            },
            start_boundary: if scope_start == minute {
                Boundary::Minute
            } else {
                Boundary::Birth
            },
            end_boundary: if scope_end == minute + 60 {
                Boundary::Minute
            } else {
                Boundary::Retirement
            },
            capture_revision: Number(revision),
            projected_revision: Number(revision),
            capture_through_ms: millis(projected_through).map_err(Domain)?,
            measured_mapping_revision: None,
            producer_hold: None,
            positions: vec![Position {
                basis: Basis::Allocated,
                stream_id: stream.id.clone(),
                sequence: Number(stream.sequence),
                digest: stream
                    .previous_digest
                    .clone()
                    .ok_or_else(|| Domain(corrupt()))?,
            }],
            timeline,
            facts,
            contributions,
        };
        statement.validate().map_err(|_| Domain(corrupt()))?;
        Ok(AllocatedBuild::Ready(Box::new(statement)))
    }

    pub(super) async fn retained_statement(
        &self,
        session: &mut ClientSession,
        source: &str,
        minute: u64,
        basis: Basis,
    ) -> Result<Option<Statement>, TransactionEventError> {
        use TransactionEventError::{Domain, Driver};
        let Some(row) = self
            .collection(INVENTORY)
            .find_one(doc! {"_id":key(&self.config.namespace,source,minute,basis)})
            .session(&mut *session)
            .await
            .map_err(Driver)?
        else {
            return Ok(None);
        };
        let bytes = row
            .get_binary_generic("canonical")
            .map_err(|_| Domain(corrupt()))?;
        if row.get_str("digest").ok() != Some(hash(bytes).as_str()) {
            return Err(Domain(corrupt()));
        }
        let value: Value = serde_json::from_slice(bytes).map_err(|_| Domain(corrupt()))?;
        let statement = Statement::from_value(value).map_err(|_| Domain(corrupt()))?;
        Ok(Some(statement))
    }

    pub(super) async fn stage_statement(
        &self,
        session: &mut ClientSession,
        source: &str,
        minute: u64,
        mut statement: Statement,
    ) -> Result<MinuteOutcome, TransactionEventError> {
        use TransactionEventError::{Domain, Driver};
        let head_id = key(
            &statement.producer_namespace,
            source,
            minute,
            statement.basis,
        );
        let rows = self.collection(INVENTORY);
        let previous = rows
            .find_one(doc! {"_id":&head_id})
            .session(&mut *session)
            .await
            .map_err(Driver)?;
        let mut comparable = serde_json::to_value(&statement).map_err(|_| Domain(corrupt()))?;
        let object = comparable
            .as_object_mut()
            .ok_or_else(|| Domain(corrupt()))?;
        for key in [
            "statement_id",
            "revision",
            "previous_statement_digest",
            "positions",
        ] {
            object.remove(key);
        }
        let semantic =
            hash(&serde_json_canonicalizer::to_vec(&comparable).map_err(|_| Domain(corrupt()))?);
        statement.revision = Number(1);
        statement.previous_statement_digest = None;
        if let Some(previous) = &previous {
            if previous.get_str("semantic").ok() == Some(semantic.as_str()) {
                return Ok(MinuteOutcome::Staged);
            }
            let revision = previous
                .get_i64("revision")
                .map_err(|_| Domain(corrupt()))?
                .checked_add(1)
                .ok_or_else(|| Domain(corrupt()))?;
            statement.revision = Number(u64::try_from(revision).map_err(|_| Domain(corrupt()))?);
            statement.previous_statement_digest = Some(
                previous
                    .get_str("digest")
                    .map_err(|_| Domain(corrupt()))?
                    .into(),
            );
        }
        statement.statement_id = uuid::Uuid::new_v4().to_string();
        statement.validate().map_err(|_| Domain(corrupt()))?;
        let bytes = serde_json_canonicalizer::to_vec(&statement).map_err(|_| Domain(corrupt()))?;
        if bytes.len() > 512 * 1024 {
            return Ok(MinuteOutcome::Held("statement_size_bound"));
        }
        let digest = hash(&bytes);
        let canonical = Binary {
            subtype: BinarySubtype::Generic,
            bytes,
        };
        rows.insert_one(doc! {"_id":outbox_id(&statement.producer_namespace,&statement.statement_id),"document_schema_version":1_i64,"canonical":&canonical,"digest":&digest,"status":"pending","generation":0_i64})
            .session(&mut *session).await.map_err(Driver)?;
        rows.replace_one(doc! {"_id":&head_id},doc! {"_id":head_id,"document_schema_version":1_i64,"canonical":canonical,"digest":digest,"semantic":semantic,"revision":number(statement.revision.0).map_err(Domain)?})
            .upsert(previous.is_none()).session(&mut *session).await.map_err(Driver)?;
        Ok(MinuteOutcome::Staged)
    }
}
