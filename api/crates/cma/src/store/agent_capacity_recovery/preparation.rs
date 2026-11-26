//! Admission preparation occurs before the sole publication transaction. A
//! preexisting admission with no proven publication is never treated as fresh.
use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::{
    resource_access::AuthorizedResource,
    sandbox::agent_capacity_recovery::{
        ports::{CommitOutcome, RecoveryError},
        state::{RecoveryRequest, RecoveryWork},
    },
};
use mongodb::bson::{spec::BinarySubtype, Binary, DateTime};

use super::{authority, transactions};
use crate::store::{
    agent_log_integrity::TransactionLogError,
    agent_log_segments::{
        capture::{self, CapturedFormat},
        publication::PublicationIdentity,
        transaction_scope::TransactionScope,
        writer_admission::{self, Admission, AdmissionKind},
        writer_bootstrap::{self, BootstrapPlan},
        writer_gate::slots,
    },
    sandbox_mongo::{start_transaction, MongoSandboxStore},
};

pub(super) struct Prepared {
    pub admission: Admission,
    pub bootstrap: Option<BootstrapPlan>,
}

pub(super) async fn prepare(
    store: &MongoSandboxStore,
    authorized: &AuthorizedResource,
    request: &RecoveryRequest,
) -> Result<CommitOutcome<Prepared>, RecoveryError> {
    let mut session = store
        .client
        .start_session()
        .await
        .map_err(|_| RecoveryError::Unavailable)?;
    let mut scope = TransactionScope::start(&mut session)
        .await
        .map_err(|_| RecoveryError::Unavailable)?;
    let source = async {
        let current = authority::capture(store, authorized, &request.agent_id, &mut scope).await?;
        let context = current.context()?;
        let captured = capture::capture_transaction(store, context.agent, &mut scope).await?;
        let failures =
            capture::points_transaction(store, &captured, &[request.failure_seq], &mut scope)
                .await?;
        let [failure] = failures.as_slice() else {
            return Err(layer2_core::sandbox::store::SandboxStoreError::Conflict.into());
        };
        let plan = context
            .plan(failure, &request.requested_at)
            .map_err(|_| layer2_core::sandbox::store::SandboxStoreError::Conflict)?;
        let work = RecoveryWork::prepare(request.clone(), &plan, &context)
            .map_err(|_| layer2_core::sandbox::store::SandboxStoreError::Conflict)?;
        let identity = work
            .admission_identity()
            .map_err(|_| layer2_core::sandbox::store::SandboxStoreError::Conflict)?;
        let operation = PublicationIdentity {
            key: identity.key,
            appended_at: identity.appended_at,
            fingerprint: Binary {
                subtype: BinarySubtype::Generic,
                bytes: identity.fingerprint.to_vec(),
            },
        };
        Ok::<_, TransactionLogError>((
            context.agent.clone(),
            operation,
            captured.format() == CapturedFormat::Legacy,
        ))
    }
    .await;
    let _ = scope.release().abort_guarded().await;
    let (agent, operation, legacy) = source
        .inspect_err(|_error| {
            #[cfg(test)]
            eprintln!("capacity recovery source preparation refused: {_error:?}");
        })
        .map_err(transactions::error)?;

    // This separate transaction admits both durable gates and claims the exact
    // preparation slot. It contains no source proof and no native operation.
    start_transaction(&mut session)
        .await
        .map_err(|_| RecoveryError::Unavailable)?;
    let database = store.client.database(&store.agents.namespace().db);
    let prepared = async {
        if writer_admission::read(&database, &agent, &operation, &mut session)
            .await?
            .is_some()
        {
            return Ok(None);
        }
        let admission = writer_admission::admit(
            &database,
            &agent,
            &operation,
            AdmissionKind::CapacityRecovery,
            &layer2_core::now_rfc3339(),
            &mut session,
        )
        .await
        .inspect_err(|_error| {
            #[cfg(test)]
            eprintln!("capacity recovery gate admission refused: {_error:?}");
        })?;
        let permit = if legacy {
            slots::claim(
                &database,
                &agent,
                &operation,
                &admission,
                DateTime::now(),
                &mut session,
            )
            .await
            .inspect_err(|_error| {
                #[cfg(test)]
                eprintln!("capacity recovery bootstrap slot refused: {_error:?}");
            })?
        } else {
            None
        };
        Ok::<_, TransactionLogError>(Some((admission, permit)))
    }
    .await;
    let (admission, permit) = match transactions::finish(&mut session, prepared).await? {
        CommitOutcome::Recorded(Some(prepared)) => prepared,
        // Missing carrier after a prior admitted attempt is unresolved. An
        // absent publication cannot prove that its transaction never committed.
        CommitOutcome::Recorded(None) | CommitOutcome::Unknown => {
            return Ok(CommitOutcome::Unknown)
        }
        CommitOutcome::KnownAbort => return Ok(CommitOutcome::KnownAbort),
    };
    let bootstrap = if legacy {
        let permit = permit.ok_or(RecoveryError::InvalidEvidence)?;
        Some(
            writer_bootstrap::stage(store, &agent, &operation, &admission, permit)
                .await
                .inspect_err(|_error| {
                    #[cfg(test)]
                    eprintln!("capacity recovery bootstrap refused: {_error:?}");
                })
                .map_err(transactions::error)?,
        )
    } else {
        None
    };
    Ok(CommitOutcome::Recorded(Prepared {
        admission,
        bootstrap,
    }))
}
