//! Prepare optional adoption before the original publication transaction.
//! Closed/absent gates leave the legacy writer and its BSON bytes unchanged.
use super::*;
use crate::store::agent_log_segments::{
    incarnation,
    writer_admission::{self, Admission, AdmissionKind},
    writer_bootstrap::BootstrapPlan,
    writer_gate::{self, GateName},
};
use crate::store::transaction_terminal::GuardedTerminal as _;

impl MongoSandboxStore {
    /// A process-local dispatch switch, never authority to admit a publication.
    pub(crate) fn with_segment_adoption_enabled(mut self, enabled: bool) -> Self {
        self.segment_adoption_enabled = enabled;
        self
    }
}

pub(in crate::store) struct Prepared {
    pub(super) admission: Admission,
    pub(super) bootstrap: BootstrapPlan,
}

pub(in crate::store) async fn prepare(
    store: &MongoSandboxStore,
    mutation: &AgentConversationMutation,
    checkpoint: Option<&AgentMessageCheckpoint>,
) -> Result<Option<Prepared>, SandboxStoreError> {
    // The default path must issue exactly the original Mongo commands, not even
    // a gate lookup or an empty transaction. Existing v2 publication is separate.
    if !store.segment_adoption_enabled {
        return Ok(None);
    }
    let mut session = store.client.start_session().await.map_err(driver)?;
    let mut scope = TransactionScope::start(&mut session)
        .await
        .map_err(driver)?;
    let result = selected(store, mutation, checkpoint, &mut scope).await;
    // Domain validation can write transactional fences. They must never escape
    // this preflight; the original root repeats them with the actual publication.
    scope.release().abort_guarded().await.map_err(driver)?;
    let Some((current, operation)) = result.map_err(domain_error)? else {
        return Ok(None);
    };
    let (admission, bootstrap) = crate::store::agent_log::keyed::preparation::prepare(
        store,
        &current,
        &operation,
        Some(AdmissionKind::Activation),
    )
    .await?;
    Ok(Some(Prepared {
        admission: admission.ok_or(SandboxStoreError::Conflict)?,
        bootstrap: bootstrap.ok_or(SandboxStoreError::Conflict)?,
    }))
}

async fn selected(
    store: &MongoSandboxStore,
    mutation: &AgentConversationMutation,
    checkpoint: Option<&AgentMessageCheckpoint>,
    scope: &mut TransactionScope<'_>,
) -> Result<Option<(Agent, publication::PublicationIdentity)>, ApplyError> {
    let database = store.client.database(&store.agents.namespace().db);
    let Some(gate) = writer_gate::read(&database, GateName::Segments, scope.session()).await?
    else {
        return Ok(None);
    };
    let row = scope
        .find_one(&store.agents, doc! {"_id":mutation.agent.id.as_str()})
        .await
        .map_err(ApplyError::Driver)?
        .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
    if read_format(&row).map_err(ApplyError::Domain)?.is_some() {
        return Ok(None);
    }
    let current = read_agent(&row).map_err(ApplyError::Domain)?;
    let operation = identity(mutation, false, checkpoint)?;
    let selected = gate.policy.selects(
        current.id.as_str(),
        current.owner.as_str(),
        &incarnation(&current),
    );
    // Closure preserves a finite, exact previously admitted operation only.
    if !selected
        && writer_admission::read(&database, &current, &operation, scope.session())
            .await?
            .is_none()
    {
        return Ok(None);
    }
    domain::validate_mutation(&current, current.conversation.last_seq, mutation)
        .map_err(ApplyError::Domain)?;
    let logs = domain::conversation_logs(mutation).map_err(ApplyError::Domain)?;
    if checkpoint
        .is_some_and(|next| !next.matches(&mutation.agent) || next.outcome.appended != logs)
    {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    domain::publication::before(store, &current, mutation, scope.session()).await?;
    domain::publication::require_cleanup(&row, &current, mutation)?;
    let captured = capture::capture_transaction(store, &current, scope).await?;
    domain::publication::bind_domain(
        store,
        &current,
        captured.projection().latest_turn,
        mutation,
        false,
        scope.session(),
    )
    .await?;
    Ok(Some((current, operation)))
}

fn driver(_: mongodb::error::Error) -> SandboxStoreError {
    SandboxStoreError::Unavailable("Agent log adoption preflight is unavailable".into())
}

fn domain_error(error: ApplyError) -> SandboxStoreError {
    match error {
        ApplyError::Domain(error) => error,
        ApplyError::Driver(error) => driver(error),
    }
}
