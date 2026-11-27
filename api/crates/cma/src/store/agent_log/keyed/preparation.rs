use super::*;
use crate::store::agent_log_segments::{
    source_keys, writer_admission, writer_bootstrap, writer_gate::slots,
};
use mongodb::bson::DateTime;

pub(in crate::store) async fn prepare(
    store: &MongoSandboxStore,
    current: &Agent,
    operation: &PublicationIdentity,
    kind: Option<AdmissionKind>,
) -> Result<(Option<Admission>, Option<BootstrapPlan>), SandboxStoreError> {
    let Some(kind) = kind else {
        return Ok((None, None));
    };
    let database = store.client.database(&store.agents.namespace().db);
    if kind == AdmissionKind::Activation {
        source_keys::require_index(&database, None)
            .await
            .map_err(domain)?;
    }
    let mut session = store.client.start_session().await.map_err(driver)?;
    start_transaction(&mut session).await.map_err(driver)?;
    let result = async {
        let admission = writer_admission::admit(
            &database,
            current,
            operation,
            kind,
            &layer2_core::now_rfc3339(),
            &mut session,
        )
        .await?;
        let permit = slots::claim(
            &database,
            current,
            operation,
            &admission,
            DateTime::now(),
            &mut session,
        )
        .await?;
        Ok::<_, TransactionLogError>((admission, permit))
    }
    .await;
    let (admission, permit) = match result {
        Ok(value) => value,
        Err(error) => {
            let _ = session.abort_guarded().await;
            return Err(domain(error));
        }
    };
    match commit_transaction(&mut session).await {
        CommitOutcome::Committed => {}
        CommitOutcome::Unknown(_) => {
            return Err(SandboxStoreError::Unavailable(
                "Agent log admission outcome is unknown; retain the original identity before retry"
                    .into(),
            ))
        }
        CommitOutcome::Retry(_) => {
            let _ = session.abort_guarded().await;
            return Err(SandboxStoreError::Conflict);
        }
        CommitOutcome::Failed(error) => return Err(driver(error)),
    }
    let bootstrap = match permit {
        Some(permit) => Some(
            writer_bootstrap::stage(store, current, operation, &admission, permit)
                .await
                .map_err(domain)?,
        ),
        None if kind == AdmissionKind::Rollover => None,
        None => return Err(SandboxStoreError::Conflict),
    };
    Ok((Some(admission), bootstrap))
}
