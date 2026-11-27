use super::*;
use contract::UnknownReason;
use layer2_core::account_deletion::subject_hash;

const SECONDS_PER_DAY: u64 = 86_400;

impl CreationOutcomes {
    pub(super) async fn resolve(
        &self,
        identity: &Identity<'_>,
        query: &LookupRequest,
    ) -> Result<Outcome, StatusCode> {
        let client = self.database.client();
        let mut session = client
            .start_session()
            .snapshot(true)
            .await
            .map_err(unavailable)?;
        // The driver keeps an inherited read concern level when it adds the
        // session's snapshot time. Supply snapshot explicitly on this handle.
        let snapshot = client.database_with_options(
            self.database.name(),
            mongodb::options::DatabaseOptions::builder()
                .read_concern(mongodb::options::ReadConcern::snapshot())
                .selection_criteria(mongodb::options::SelectionCriteria::ReadPreference(
                    mongodb::options::ReadPreference::Primary,
                ))
                .build(),
        );
        self.resolve_snapshot(&snapshot, identity, query, &mut session)
            .await
    }

    async fn resolve_snapshot(
        &self,
        snapshot: &Database,
        identity: &Identity<'_>,
        query: &LookupRequest,
        session: &mut ClientSession,
    ) -> Result<Outcome, StatusCode> {
        let Some(period) = identity.period().map_err(identity_error)? else {
            return self.legacy(snapshot, identity, query, session).await;
        };
        let records = snapshot.collection(COLLECTION);
        if let Some(row) = Self::read_record(&records, identity, session).await? {
            if query.claim_id.is_some() {
                return Err(StatusCode::CONFLICT);
            }
            if query
                .operation_id
                .as_ref()
                .is_some_and(|id| row.operation_id.as_ref() != Some(id))
            {
                return Err(StatusCode::CONFLICT);
            }
            return match row.state {
                Evidence::Created { sandbox_id }
                    if valid_sandbox_id(&sandbox_id)
                        && (identity.operation == Operation::Sandbox
                            || row.operation_id.is_some()) =>
                {
                    Ok(Outcome::created(
                        &row.identity,
                        &row.generation,
                        sandbox_id,
                        row.operation_id,
                    ))
                }
                Evidence::Created { .. } => Err(StatusCode::SERVICE_UNAVAILABLE),
                Evidence::Refused { code } => Ok(Outcome::refused(
                    &row.identity,
                    &row.generation,
                    code,
                    row.operation_id,
                )),
                Evidence::Pending if period.epoch == self.config.epoch => {
                    Self::pending(snapshot, &row, session).await
                }
                Evidence::Pending => Ok(unknown(UnknownReason::CoverageUnavailable)),
            };
        }
        if query.claim_id.is_some() || query.operation_id.is_some() {
            return Ok(unknown(UnknownReason::NoDurableEvidence));
        }
        if self.is_closed(&records, &period, session).await?
            && owner_day_covered(snapshot, identity.owner, period.day, session).await?
        {
            Ok(Outcome::never_received(&identity.digest(), &period))
        } else {
            Ok(unknown(UnknownReason::CoverageUnavailable))
        }
    }

    async fn pending(
        snapshot: &Database,
        row: &Record,
        session: &mut ClientSession,
    ) -> Result<Outcome, StatusCode> {
        if let Some(id) = &row.operation_id {
            let operation = snapshot
                .collection::<Document>(super::super::agent_lifecycle_operations::COLLECTION)
                .find_one(doc! {"_id": id, "owner": &row.owner})
                .projection(doc! {"status":1,"document_schema_version":1})
                .max_time(IO_BUDGET)
                .session(session)
                .await
                .map_err(unavailable)?
                .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
            schema::require_current(
                operation
                    .get_i64("document_schema_version")
                    .map_err(unavailable)?,
            )
            .map_err(unavailable)?;
            return match operation.get_str("status").map_err(unavailable)? {
                "queued" | "running" | "ready" | "waiting_for_connection" => {
                    Ok(Outcome::InProgress {
                        operation_id: Some(id.clone()),
                    })
                }
                "failed" => Ok(unknown(UnknownReason::RecoveryRequired)),
                _ => Err(StatusCode::SERVICE_UNAVAILABLE),
            };
        }
        if row.lease_until > DateTime::now() {
            Ok(Outcome::InProgress { operation_id: None })
        } else {
            Ok(unknown(UnknownReason::RecoveryRequired))
        }
    }
}

pub(super) async fn owner_day_covered(
    database: &Database,
    owner: &SubjectId,
    day: i64,
    session: &mut ClientSession,
) -> Result<bool, StatusCode> {
    let row = database
        .collection::<Document>(super::super::account_deletions::RECORDS)
        .find_one(doc! {"subject_hash":subject_hash(owner)})
        .sort(doc! {"requested_at":-1})
        .projection(doc! {"requested_at":1,"status":1,"document_schema_version":1})
        .max_time(IO_BUDGET)
        .session(session)
        .await
        .map_err(unavailable)?;
    let Some(row) = row else { return Ok(true) };
    let version = row
        .get_i64("document_schema_version")
        .map_err(unavailable)?;
    schema::require_current(version).map_err(unavailable)?;
    if row.get_str("status").map_err(unavailable)? != "completed" {
        return Ok(false);
    }
    let barrier_day =
        layer2_core::seconds_from_rfc3339(row.get_str("requested_at").map_err(unavailable)?)
            .and_then(|seconds| i64::try_from(seconds / SECONDS_PER_DAY).ok())
            .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    Ok(day > barrier_day)
}

pub(super) fn unknown(reason: UnknownReason) -> Outcome {
    Outcome::Unknown { reason }
}

pub(super) fn valid_sandbox_id(id: &str) -> bool {
    id.strip_prefix("sbx_").is_some_and(|suffix| {
        !suffix.is_empty()
            && suffix.len() <= 64
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    })
}

pub(super) fn outcome_name(result: &Result<Outcome, StatusCode>) -> &'static str {
    match result {
        Ok(Outcome::Created { .. }) => "created",
        Ok(Outcome::Refused { .. }) => "refused",
        Ok(Outcome::NeverReceived { .. }) => "never_received",
        Ok(Outcome::InProgress { .. }) => "in_progress",
        Ok(Outcome::Unknown { .. }) => "unknown",
        Err(_) => "error",
    }
}
