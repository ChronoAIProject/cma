//! Adapt command roots without exposing the scoped transaction capability.
//! Invoked domain functions never start, restart, commit or abort transactions.
use super::{terminal, transaction_scope::TransactionScope};
use crate::store::{
    agent_commands::{self as commands, worker::Change, ApplyError},
    agent_log_integrity::TransactionLogError,
    sandbox_mongo::MongoSandboxStore,
};
use layer2_core::sandbox::agent_io::{
    AdmittedAgentCommand, CommandFailureCode, CommandLedgerError, QueuedCancellationOutcome,
    QueuedCancellationReservation,
};

pub(in crate::store) async fn worker(
    store: &MongoSandboxStore,
    change: &Change,
    scope: &mut TransactionScope<'_>,
) -> Result<
    (
        commands::worker::TransactionResult,
        Option<terminal::Receipt>,
    ),
    ApplyError,
> {
    let result = Box::pin(commands::worker_apply::apply(
        store,
        change,
        scope.session(),
    ))
    .await?;
    let receipt = match (change, result.0.as_ref()) {
        (Change::Reject { now, .. }, Some(command)) => {
            Box::pin(terminal::project(store, command, now, scope))
                .await
                .map_err(error)?
        }
        _ => None,
    };
    Ok((result, receipt))
}

pub(in crate::store) async fn cancel(
    store: &MongoSandboxStore,
    request: &QueuedCancellationReservation,
    scope: &mut TransactionScope<'_>,
) -> Result<(QueuedCancellationOutcome, Option<terminal::Receipt>), ApplyError> {
    let result = Box::pin(commands::queued_cancellation::apply::apply(
        store,
        request,
        scope.session(),
    ))
    .await?;
    let receipt = match &result {
        QueuedCancellationOutcome::Applied(result) => {
            let mut receipt = Box::pin(terminal::project(
                store,
                &result.response,
                &request.command.admitted_at,
                scope,
            ))
            .await
            .map_err(error)?;
            if let Some(receipt) = &mut receipt {
                receipt.related = Some(scope.find_one(&store.agent_commands,
                    mongodb::bson::doc! {"_id":result.record.receipt.envelope.command_id.as_str()})
                    .await?.ok_or(CommandLedgerError::Corrupt)?);
            }
            receipt
        }
        QueuedCancellationOutcome::NotAdmitted => None,
    };
    Ok((result, receipt))
}

pub(in crate::store) async fn refuse(
    store: &MongoSandboxStore,
    expected: &AdmittedAgentCommand,
    revision: u64,
    code: CommandFailureCode,
    now: String,
    scope: &mut TransactionScope<'_>,
) -> Result<Option<terminal::Receipt>, ApplyError> {
    let command = commands::unclaimed::apply(
        store,
        expected,
        revision,
        code,
        now.clone(),
        scope.session(),
    )
    .await?;
    Box::pin(terminal::project(store, &command, &now, scope))
        .await
        .map_err(error)
}

fn error(error: TransactionLogError) -> ApplyError {
    match error {
        TransactionLogError::Driver(error) => ApplyError::Driver(error),
        TransactionLogError::Domain(
            layer2_core::sandbox::store::SandboxStoreError::Unavailable(_),
        ) => ApplyError::Domain(CommandLedgerError::Unavailable),
        TransactionLogError::Domain(_) => ApplyError::Domain(CommandLedgerError::Conflict),
    }
}
