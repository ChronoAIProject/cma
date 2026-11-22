//! Shared complete cursor traversal with an additive admitted composition.
use super::*;
use crate::store::agent_log_telemetry::Scan;
use crate::store::ingress_reads::error::ReadFailure;
use futures_util::TryStreamExt;
use layer2_core::sandbox::ingress_ownership::{
    sizing::shared_owner_bytes, AdmittedError, AdmittedOperation, AllocationPlan, IngressClass,
    IngressOwnershipPolicy, OwnedWork, SharedOwned,
};
use std::future::IntoFuture;

#[derive(Clone, Copy)]
pub(in crate::store::agent_log_integrity) struct ScanInput<'a> {
    pub(in crate::store::agent_log_integrity) collection: &'a mongodb::Collection<Document>,
    pub(in crate::store::agent_log_integrity) agent_id: &'a AgentId,
    pub(in crate::store::agent_log_integrity) last_seq: u64,
}

pub(super) async fn visit_raw_rows<F, C: Clock, E>(
    input: ScanInput<'_>,
    session: &mut mongodb::ClientSession,
    snapshot: bool,
    raw_batches: bool,
    observation: &mut Scan<C>,
    consume: &mut F,
    map_driver: impl Fn(mongodb::error::Error) -> E,
) -> Result<(), E>
where
    F: FnMut(&mongodb::bson::RawDocument, &mut Scan<C>) -> Result<(), E>,
{
    let raw_log = input.collection.clone_with_type::<RawDocumentBuf>();
    let find = raw_log
        .find(mongodb::bson::doc! { "agent_id": input.agent_id.as_str() })
        .sort(mongodb::bson::doc! { "seq": 1 })
        .limit(i64::try_from(PHYSICAL_AGENT_LOG_ENTRIES + 1).unwrap_or(i64::MAX));
    let find = if snapshot {
        find.read_concern(ReadConcern::snapshot())
    } else {
        find
    };
    // Both paths observe initial-find suspension, but only cursor advancement
    // contributes to driver-poll timing. Keep the session and query unchanged.
    if raw_batches {
        let mut cursor = observation
            .observe(find.session(&mut *session).batch(), false)
            .await
            .map_err(&map_driver)?;
        while let Some(batch) = observation
            .observe(cursor.stream(session).try_next(), true)
            .await
            .map_err(&map_driver)?
        {
            // The ordinary cursor copies every document in a ready batch before
            // returning a row. Borrowing lets checkpoints separate that work;
            // it needs neither smaller server batches nor additional checkouts.
            for item in batch.doc_slices().map_err(&map_driver)? {
                consume(batch_document(item).map_err(&map_driver)?, observation)?;
                observation.checkpoint().await;
            }
        }
    } else {
        let mut cursor = observation
            .observe(find.session(&mut *session).into_future(), false)
            .await
            .map_err(&map_driver)?;
        while let Some(raw) = observation
            .observe(cursor.stream(session).try_next(), true)
            .await
            .map_err(&map_driver)?
        {
            consume(raw.as_ref(), observation)?;
            observation.checkpoint().await;
        }
    }
    Ok(())
}

#[derive(Debug)]
enum ScanFailure {
    Admission(AdmittedError),
    Read(OwnedWork<ReadFailure>),
}

type ScanRead<'a> = AdmittedOperation<'a, Result<LogSummary, ReadFailure>>;

/// The plan covers query/raw/cursor/error storage; driver bounds and detached
/// cleanup remain unqualified. Callback captures and input provenance remain
/// with the caller. Every failure discards the private partial scan state.
pub(in crate::store::agent_log_integrity) fn scan_collection_admitted<'a, F, C>(
    caller: ScanCaller,
    input: ScanInput<'a>,
    session: &'a mut mongodb::ClientSession,
    options: ScanOptions<C>,
    policy: &'a IngressOwnershipPolicy,
    read_plan: AllocationPlan,
    visit: F,
) -> Result<ScanRead<'a>, AdmittedError>
where
    C: Clock + 'a,
    F: FnMut(SharedOwned<AgentLogEntry>) -> Result<(), OwnedWork<SandboxStoreError>> + Send + 'a,
{
    scan_rows_admitted(
        caller,
        input,
        session,
        ScanMode {
            options,
            known_corrupt: false,
        },
        policy,
        read_plan,
        visit,
    )
}

pub(in crate::store::agent_log_integrity) fn scan_corruption_admitted<'a>(
    caller: ScanCaller,
    input: ScanInput<'a>,
    session: &'a mut mongodb::ClientSession,
    options: ScanOptions,
    policy: &'a IngressOwnershipPolicy,
    read_plan: AllocationPlan,
) -> Result<ScanRead<'a>, AdmittedError> {
    scan_rows_admitted(
        caller,
        input,
        session,
        ScanMode {
            options,
            known_corrupt: true,
        },
        policy,
        read_plan,
        |_| Ok(()),
    )
}

fn scan_rows_admitted<'a, F, C>(
    caller: ScanCaller,
    input: ScanInput<'a>,
    session: &'a mut mongodb::ClientSession,
    mode: ScanMode<C>,
    policy: &'a IngressOwnershipPolicy,
    read_plan: AllocationPlan,
    mut visit: F,
) -> Result<ScanRead<'a>, AdmittedError>
where
    C: Clock + 'a,
    F: FnMut(SharedOwned<AgentLogEntry>) -> Result<(), OwnedWork<SandboxStoreError>> + Send + 'a,
{
    let ScanMode {
        options,
        known_corrupt,
    } = mode;
    AdmittedOperation::from_owned(
        policy,
        IngressClass::Transformation,
        read_plan,
        move |scope| async move {
            let mut observation = Scan::with_clock(caller, options.cooperation, options.clock);
            if let Err(error) = rows::require_head(input.last_seq) {
                return Ok(scope.initialize_owned(|| Err(ReadFailure::Domain(error))));
            }
            let retained = policy.admit(
                IngressClass::Data,
                AllocationPlan::checked(
                    "retained complete scan summary",
                    &[size_of::<LogSummary>(), shared_owner_bytes::<()>()],
                )?,
            )?;
            let mut read = crate::store::mongo_read_telemetry::Read::find(input.collection.name());
            let mut state = rows::ScanState::default();
            let result = visit_raw_rows(
                input,
                session,
                options.snapshot,
                options.raw_batches,
                &mut observation,
                &mut |raw: &mongodb::bson::RawDocument, observation: &mut Scan<C>| {
                    observation.document(raw.as_bytes().len());
                    read.document(raw.as_bytes().len());
                    rows::consume_row_owned(
                        &mut state,
                        input.last_seq,
                        raw,
                        policy,
                        &scope,
                        &mut visit,
                    )
                    .map_err(ScanFailure::Admission)?
                    .transpose()
                    .map(|_| ())
                    .map_err(ScanFailure::Read)
                },
                |error| ScanFailure::Read(scope.initialize_owned(|| ReadFailure::Driver(error))),
            )
            .await;
            match result {
                Err(ScanFailure::Admission(error)) => return Err(error),
                Err(ScanFailure::Read(error)) => return Ok(error.map(Err)),
                Ok(()) => {}
            }
            let summary = match state.finish(input.last_seq) {
                Ok(summary) => summary,
                Err(error) => return Ok(scope.initialize_owned(|| Err(ReadFailure::Domain(error)))),
            };
            let result = scope
                .initialize_owned(|| Ok(summary))
                .retain_admitted(retained)?;
            if !known_corrupt {
                observation.complete();
            }
            read.complete();
            Ok(result)
        },
    )
}

#[cfg(test)]
#[path = "scan_cursor_ingress_tests.rs"]
mod tests;
