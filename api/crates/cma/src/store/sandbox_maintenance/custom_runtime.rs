//! Shared scoped admission for ordinary and custom fleet rotations.
use super::{
    transaction::{ApplyError, MaintenanceRequest},
    MongoSandboxStore,
};
use layer2_core::sandbox::store::{
    runtime_migration_now_ms, RuntimeMigrationAdmissionBasis, RuntimeMigrationClearance,
    SandboxCustomRuntimeMigrationAdmission, SandboxImageRotationRequest, SandboxMaintenanceOutcome,
    SandboxStore, SandboxStoreError,
};
use layer2_core::sandbox::{
    Agent, RuntimeMigrationAuthorityEntry, RuntimeMigrationIdentity, Sandbox,
    SandboxLifecycleOperation,
};
use mongodb::{
    bson::{doc, Document},
    ClientSession,
};

pub(super) async fn admit(
    store: &MongoSandboxStore,
    mut admission: SandboxCustomRuntimeMigrationAdmission,
) -> Result<Sandbox, SandboxStoreError> {
    let current = store
        .get_sandbox(admission.sandbox_id())
        .await?
        .ok_or(SandboxStoreError::NotFound)?;
    if admission.is_replay(&current)? {
        return Ok(current);
    }
    if !store.runtime_migration_enabled || store.renewable_work || store.agent_ownership.is_some() {
        return Err(SandboxStoreError::InvalidTransaction(
            "runtime migration requires qualified single-process authority",
        ));
    }
    admission.basis.admitted_at = layer2_core::now_rfc3339();
    let receipt =
        super::schedule_receipt(store, MaintenanceRequest::Custom(Box::new(admission))).await?;
    match receipt.outcome {
        SandboxMaintenanceOutcome::Scheduled | SandboxMaintenanceOutcome::AlreadyScheduled => {
            Ok(receipt.sandbox)
        }
        SandboxMaintenanceOutcome::NotEligible => Err(SandboxStoreError::Conflict),
    }
}

pub(super) async fn schedule_rotation(
    store: &MongoSandboxStore,
    request: SandboxImageRotationRequest,
) -> Result<SandboxMaintenanceOutcome, SandboxStoreError> {
    let Some(authority) = store
        .runtime_migration_authority
        .as_ref()
        .filter(|authority| authority.policy().includes(&request.sandbox_id))
    else {
        return super::schedule(store, MaintenanceRequest::Rotation(request)).await;
    };
    let current = store
        .get_sandbox(&request.sandbox_id)
        .await?
        .ok_or(SandboxStoreError::NotFound)?;
    if matches!(&current.lifecycle.operation, Some(SandboxLifecycleOperation::ImageRotation { target_runtime_version, .. }) if target_runtime_version == &request.expected_runtime_version)
    {
        return Ok(SandboxMaintenanceOutcome::AlreadyScheduled);
    }
    if !store.runtime_migration_enabled || store.renewable_work || store.agent_ownership.is_some() {
        return Err(SandboxStoreError::Conflict);
    }
    let entry = authority.resolve(&request.sandbox_id).await?;
    authority
        .policy()
        .validate_entry(&entry, runtime_migration_now_ms()?)?;
    if entry.custom_migration.is_some()
        || entry.identity != RuntimeMigrationIdentity::capture(&current)?
        || entry.target_image != request.expected_runtime_version
    {
        return Err(SandboxStoreError::Conflict);
    }
    let basis = RuntimeMigrationAdmissionBasis {
        authority_id: entry.authority_id,
        policy_id: authority.policy().policy_id.clone(),
        request_key: entry.request_key,
        identity: entry.identity.clone(),
        authority_issued_at_ms: entry.issued_at_ms,
        authority_expires_at_ms: entry.expires_at_ms,
        approved_target: entry.target_image,
        source_runtime: entry.identity.runtime,
        clearance: entry.clearance,
        admitted_at: layer2_core::now_rfc3339(),
        previous_request_keys: Vec::new(),
        earlier_admissions: Vec::new(),
    };
    super::schedule(
        store,
        MaintenanceRequest::FleetRotation(request, Box::new(basis)),
    )
    .await
}

pub(super) async fn validate_admission(
    store: &MongoSandboxStore,
    document: &Document,
    current: &Sandbox,
    agents: &[Agent],
    basis: &RuntimeMigrationAdmissionBasis,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    let now_ms = runtime_migration_now_ms().map_err(ApplyError::Domain)?;
    // This internal port accepts typed data, so also enforce the installed
    // policy here. Exact durable replay has already returned before this point;
    // admission never reloads or renews a bundle during transaction retries.
    let policy = store
        .runtime_migration_authority
        .as_ref()
        .map(|authority| authority.policy())
        .filter(|policy| policy.policy_id == basis.policy_id)
        .ok_or(ApplyError::Domain(SandboxStoreError::Conflict))?;
    policy
        .validate_entry(
            &RuntimeMigrationAuthorityEntry {
                authority_id: basis.authority_id.clone(),
                request_key: basis.request_key.clone(),
                identity: basis.identity.clone(),
                target_image: basis.approved_target.clone(),
                custom_migration: None,
                clearance: basis.clearance.clone(),
                observation_reference: None,
                issued_at_ms: basis.authority_issued_at_ms,
                expires_at_ms: basis.authority_expires_at_ms,
            },
            now_ms,
        )
        .map_err(ApplyError::Domain)?;
    basis.validate_at(now_ms).map_err(ApplyError::Domain)?;
    if basis.identity != RuntimeMigrationIdentity::capture(current).map_err(ApplyError::Domain)? {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    match &basis.clearance {
        RuntimeMigrationClearance::Paused
            if current.status == layer2_core::sandbox::SandboxStatus::Paused => {}
        RuntimeMigrationClearance::Observed { proof } => {
            super::runtime_activity::validate_for_admission(
                store, document, agents, proof, now_ms, session,
            )
            .await?;
        }
        _ => return Err(ApplyError::Domain(SandboxStoreError::Conflict)),
    }
    if super::evidence::queued_control(store, &current.id, session).await?
        || !super::evidence::sessions::clear(
            store,
            &current.id,
            &layer2_core::now_rfc3339(),
            session,
        )
        .await?
    {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    // Idle Agent projections can lag durable command admission. Check all
    // commands, including those attached to terminal Agent records.
    for agent in agents {
        if store.agent_commands.find_one(doc! {"agent_id":agent.id.as_str(),"status":{"$in":["queued","in_progress","waiting_action"]}})
            .session(&mut *session).await.map_err(ApplyError::Driver)?.is_some() {
            return Err(ApplyError::Domain(SandboxStoreError::Conflict));
        }
    }
    Ok(())
}

impl MongoSandboxStore {
    pub(crate) fn with_runtime_migration_authority(
        mut self,
        authority: std::sync::Arc<dyn layer2_core::sandbox::RuntimeMigrationAuthority>,
        verifier: std::sync::Arc<layer2_core::sandbox::RuntimeActivityVerifier>,
        enabled: bool,
    ) -> Self {
        self.runtime_migration_enabled = enabled;
        self.runtime_migration_authority = Some(authority);
        self.runtime_migration_verifier = Some(verifier);
        self
    }
    pub(crate) fn runtime_migration_authority(
        &self,
    ) -> Option<std::sync::Arc<dyn layer2_core::sandbox::RuntimeMigrationAuthority>> {
        self.runtime_migration_authority.clone()
    }
    pub(crate) fn runtime_migration_verifier(
        &self,
    ) -> Option<std::sync::Arc<layer2_core::sandbox::RuntimeActivityVerifier>> {
        self.runtime_migration_verifier.clone()
    }
}
