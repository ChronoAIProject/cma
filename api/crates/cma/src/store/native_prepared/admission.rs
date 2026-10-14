//! Explicit lifecycle creation alone consumes capacity, atomically with ownership.
use super::{document, model::Ownership, read, NativePreparedPlan};
use crate::store::sandbox_admission::AdmissionError;
use crate::store::sandbox_mongo::MongoSandboxStore;
use layer2_core::profiles::WorkspaceProfileSnapshot;
use layer2_core::sandbox::{Sandbox, SandboxOrigin, SandboxStoreError};
use mongodb::bson::{doc, Bson};

pub(in crate::store) async fn claim_ready(
    store: &MongoSandboxStore,
    sandbox: &mut Sandbox,
    at: &str,
    session: &mut mongodb::ClientSession,
) -> Result<bool, AdmissionError> {
    let SandboxOrigin::Lifecycle { operation_id } = &sandbox.origin else {
        return Ok(false);
    };
    let Some(target) = store
        .native_prepared
        .find_one(doc! {"_id":"target","target":{"$gt":0}})
        .session(&mut *session)
        .await
        .map_err(AdmissionError::Driver)?
    else {
        return Ok(false);
    };
    let desired: NativePreparedPlan =
        mongodb::bson::deserialize_from_bson(target.get("plan").cloned().unwrap_or(Bson::Null))
            .map_err(|e| domain(SandboxStoreError::Corrupt(e.to_string())))?;
    desired.validate().map_err(domain)?;
    let database = store.client.database(&store.sandboxes.namespace().db);
    let Some(operation)=database.collection::<crate::store::agent_lifecycle_operations::LifecycleRecord>(crate::store::agent_lifecycle_operations::COLLECTION)
        .find_one(doc!{"_id":operation_id,"owner":sandbox.owner.as_str(),"status":{"$in":["queued","running"]}})
        .session(&mut *session).await.map_err(AdmissionError::Driver)? else{return Ok(false)};
    if operation.input.behavior != crate::store::LifecycleBehavior::PromptRepository
        || operation.input.request.workspace_id.is_some()
        || operation.input.request.sandbox_id.is_some()
        || operation.sandbox_id.is_some()
        || operation.agent_id.is_some()
    {
        return Ok(false);
    }
    let agent = crate::agent_lifecycle::validation::agent_content_for(
        &operation.input.request.profile.clone().unwrap_or_default(),
        operation.input.behavior,
    )
    .map_err(|e| domain(SandboxStoreError::Corrupt(e.to_string())))?;
    let [reference] = sandbox.profile.content.workspace_profiles.as_slice() else {
        return Ok(false);
    };
    let Some(workspace) = database
        .collection::<mongodb::bson::Document>(
            crate::store::sandbox_mongo::WORKSPACE_PROFILE_REVISIONS,
        )
        .find_one(
            doc! {"profile_id":reference.id.as_str(),"revision":i64::from(reference.revision)},
        )
        .session(&mut *session)
        .await
        .map_err(AdmissionError::Driver)?
    else {
        return Ok(false);
    };
    let revision = crate::store::workspace_profiles::read_revision(&workspace)
        .map_err(|e| domain(SandboxStoreError::Corrupt(e.to_string())))?;
    let workspace = WorkspaceProfileSnapshot::from(&revision);
    if !desired.matches(
        &sandbox.profile,
        &workspace,
        &agent,
        &sandbox.credential.reaches,
    ) {
        return Ok(false);
    }
    let seconds =
        layer2_core::seconds_from_rfc3339(at).ok_or_else(|| domain(SandboxStoreError::Conflict))?;
    let cutoff = layer2_core::rfc3339_at(
        std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds.saturating_sub(45)),
    );
    let Some(candidate)=store.native_prepared.find_one(doc!{"kind":"reservation","state":"ready","lease_until":{"$lte":at},"record.payload.updated_at":{"$gte":cutoff,"$lte":at}})
        .sort(doc!{"_id":1}).session(&mut *session).await.map_err(AdmissionError::Driver)? else{return Ok(false)};
    let mut record = read(&candidate).map_err(domain)?;
    if record.plan != desired {
        return Ok(false);
    }
    // Serialize with target changes/retirement. A stale selected pool cannot be consumed.
    store
        .native_prepared
        .update_one(doc! {"_id":"target"}, doc! {"$inc":{"fence":1_i64}})
        .session(&mut *session)
        .await
        .map_err(AdmissionError::Driver)?;
    let ownership = Ownership {
        owner: sandbox.owner.clone(),
        operation_id: operation_id.clone(),
        sandbox: sandbox.profile.clone(),
        workspace,
        agent_content: agent,
        agent: None,
    };
    // Reuse the domain's guarded ownership transfer with its immutable preparation
    // snapshot, then retain the caller's actual private profile on the owned row.
    let mut projected = sandbox.clone();
    projected.profile = record.plan.warm.profile.clone();
    record
        .payload
        .claim(&mut projected, &record.plan.warm.runtime_version, at)
        .map_err(domain)?;
    projected.profile = ownership.sandbox.clone();
    projected
        .validate()
        .map_err(|e| domain(SandboxStoreError::Corrupt(e.to_string())))?;
    record.ownership = Some(ownership);
    let result=store.native_prepared.replace_one(doc!{"_id":record.payload.id.as_str(),"state":"ready","revision":candidate.get("revision").cloned()},document(&record).map_err(domain)?)
        .session(&mut *session).await.map_err(AdmissionError::Driver)?;
    if result.matched_count != 1 {
        return Err(domain(SandboxStoreError::Conflict));
    }
    *sandbox = projected;
    Ok(true)
}
fn domain(error: SandboxStoreError) -> AdmissionError {
    AdmissionError::Domain(error)
}
