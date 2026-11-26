//! Explicit lifecycle creation alone consumes capacity, atomically with ownership.
use super::{
    document,
    model::{Ownership, Record},
    read, NativePreparedPlan,
};
use crate::store::sandbox_admission::AdmissionError;
use crate::store::sandbox_mongo::MongoSandboxStore;
use futures_util::TryStreamExt;
use layer2_core::profiles::WorkspaceProfileSnapshot;
use layer2_core::sandbox::{Sandbox, SandboxOrigin, SandboxStoreError};
use mongodb::bson::{doc, Bson, Document};

#[derive(Debug, PartialEq, Eq)]
pub(in crate::store) enum ClaimOutcome {
    Claimed,
    Miss(&'static str),
    Busy,
}

pub(in crate::store) async fn claim_ready(
    store: &MongoSandboxStore,
    sandbox: &mut Sandbox,
    at: &str,
    session: &mut mongodb::ClientSession,
) -> Result<ClaimOutcome, AdmissionError> {
    let SandboxOrigin::Lifecycle { operation_id } = &sandbox.origin else {
        return Ok(ClaimOutcome::Miss("miss_ineligible_origin"));
    };
    let machine = sandbox
        .profile
        .content
        .environment_variables
        .iter()
        .any(|e| e.name == "CMA_MACHINE_INCLUDED" && e.value == "1");
    if store.default_pool_holds.lane(machine) {
        return Ok(ClaimOutcome::Miss("miss_default_pool_held"));
    }
    let target_id = if machine { "machine-target" } else { "target" };
    let Some(target) = store
        .native_prepared
        .find_one(doc! {"_id":target_id,"target":{"$gt":0}})
        .session(&mut *session)
        .await
        .map_err(AdmissionError::Driver)?
    else {
        return Ok(ClaimOutcome::Miss("miss_target_disabled"));
    };
    let desired: NativePreparedPlan =
        mongodb::bson::deserialize_from_bson(target.get("plan").cloned().unwrap_or(Bson::Null))
            .map_err(|e| domain(SandboxStoreError::Corrupt(e.to_string())))?;
    desired.validate().map_err(domain)?;
    // The preparation worker can still have a prior release as its desired
    // plan. Use independently captured authority for a fresh transfer.
    let release = store.released_runtime();
    let machine_release = release.machine.as_deref();
    let agent_release = (!release.agent.is_empty()).then_some(release.agent.as_str());
    let released = if machine {
        machine_release
    } else {
        agent_release
    };
    if desired.machine() != machine
        || (machine && released.is_none())
        || released.is_some_and(|version| version != desired.warm.runtime_version)
    {
        return Ok(ClaimOutcome::Miss("miss_released_runtime_mismatch"));
    }
    // A target written by a process with an earlier home renderer names homes
    // whose replay the runtime would refuse. Provision normally until this
    // process's preparation worker replaces that target.
    if !desired.renders_current_home() {
        return Ok(ClaimOutcome::Miss("miss_prepared_home_rendering"));
    }
    let database = store.client.database(&store.sandboxes.namespace().db);
    let Some(operation)=database.collection::<crate::store::agent_lifecycle_operations::LifecycleRecord>(crate::store::agent_lifecycle_operations::COLLECTION)
        .find_one(doc!{"_id":operation_id,"owner":sandbox.owner.as_str(),"status":{"$in":["queued","running"]}})
        .session(&mut *session).await.map_err(AdmissionError::Driver)? else{return Ok(ClaimOutcome::Miss("miss_operation_unavailable"))};
    if !operation.input.behavior.uses_repository_profile()
        || operation.input.request.machine_included != machine
        || (machine
            && operation.admitted_machine_image() != Some(desired.warm.runtime_version.as_str()))
        || operation.input.request.workspace_id.is_some()
        || operation.input.request.sandbox_id.is_some()
        || operation.sandbox_id.is_some()
        || operation.agent_id.is_some()
    {
        return Ok(ClaimOutcome::Miss("miss_ineligible_request"));
    }
    let agent = crate::agent_lifecycle::validation::agent_content_for(
        &crate::agent_lifecycle::guidance::profile_with_instructions(
            operation.input.request.profile.clone().unwrap_or_default(),
            operation.input.managed_agent_instructions.as_deref(),
        ),
        operation.input.behavior,
    )
    .map_err(|e| domain(SandboxStoreError::Corrupt(e.to_string())))?;
    let [reference] = sandbox.profile.content.workspace_profiles.as_slice() else {
        return Ok(ClaimOutcome::Miss("miss_workspace_reference"));
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
        return Ok(ClaimOutcome::Miss("miss_workspace_profile_missing"));
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
        return Ok(ClaimOutcome::Miss("miss_profile_mismatch"));
    }
    let (candidate, mut record) = match ready_candidate(store, &desired, at, session).await? {
        Candidate::Ready(candidate) => *candidate,
        Candidate::Unavailable(outcome) => return Ok(outcome),
    };
    // Serialize with target changes/retirement. A stale selected pool cannot be consumed.
    store
        .native_prepared
        .update_one(doc! {"_id":target_id}, doc! {"$inc":{"fence":1_i64}})
        .session(&mut *session)
        .await
        .map_err(AdmissionError::Driver)?;
    // A machine lifecycle receipt already admitted its exact image. A plain
    // prepared transfer makes the first durable image choice here.
    if !machine && operation.input.resolved_image.is_none() {
        store
            .fence_released_admission(
                &release,
                layer2_core::released_runtime::ReleasedLane::Agent,
                session,
            )
            .await
            .map_err(domain)?;
    }
    let authorized_version = operation
        .input
        .resolved_image
        .as_deref()
        .or(released)
        .unwrap_or(desired.warm.runtime_version.as_str());
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
        .claim(&mut projected, authorized_version, at)
        .map_err(domain)?;
    projected.profile = ownership.sandbox.clone();
    projected
        .validate()
        .map_err(|e| domain(SandboxStoreError::Corrupt(e.to_string())))?;
    record.ownership = Some(ownership);
    let mut replacement = document(&record).map_err(domain)?;
    super::workspace_proof::retain(&mut replacement, &candidate, &record).map_err(domain)?;
    let result=store.native_prepared.replace_one(doc!{"_id":record.payload.id.as_str(),"state":"ready","revision":candidate.get("revision").cloned(),"lease_until":{"$lte":at}},replacement)
        .session(&mut *session).await.map_err(AdmissionError::Driver)?;
    if result.matched_count != 1 {
        return Err(domain(SandboxStoreError::Conflict));
    }
    *sandbox = projected;
    Ok(ClaimOutcome::Claimed)
}

enum Candidate {
    Ready(Box<(Document, Record)>),
    Unavailable(ClaimOutcome),
}

async fn ready_candidate(
    store: &MongoSandboxStore,
    desired: &NativePreparedPlan,
    observed_at: &str,
    session: &mut mongodb::ClientSession,
) -> Result<Candidate, AdmissionError> {
    let now = layer2_core::seconds_from_rfc3339(observed_at)
        .ok_or_else(|| domain(SandboxStoreError::Conflict))?;
    // Each class has a cap of two. Check both before declaring a busy miss:
    // one health probe must not hide another exact, available reservation.
    let channel = if desired.machine() {
        Bson::Boolean(true)
    } else {
        doc! {"$ne":true}.into()
    };
    let mut cursor = store
        .native_prepared
        .find(doc! {"kind":"reservation","state":"ready","machine":channel})
        .sort(doc! {"_id":1})
        .limit(2)
        .session(&mut *session)
        .await
        .map_err(AdmissionError::Driver)?;
    let candidates = cursor
        .stream(&mut *session)
        .try_collect::<Vec<_>>()
        .await
        .map_err(AdmissionError::Driver)?;
    let mut reason = "miss_no_ready_capacity";
    let mut busy = false;
    for candidate in candidates {
        let record = read(&candidate).map_err(domain)?;
        if record.plan != *desired {
            reason = "miss_reservation_plan_mismatch";
            continue;
        }
        let updated = layer2_core::seconds_from_rfc3339(&record.payload.updated_at)
            .ok_or_else(|| domain(SandboxStoreError::Conflict))?;
        let created = layer2_core::seconds_from_rfc3339(&record.payload.created_at)
            .ok_or_else(|| domain(SandboxStoreError::Conflict))?;
        if updated > now || now.saturating_sub(updated) > 45 {
            reason = "miss_stale_ready_proof";
            continue;
        }
        if now.saturating_sub(created) >= super::planning::MAX_AGE_SECONDS {
            reason = "miss_expired_reservation";
            continue;
        }
        let until = candidate
            .get_str("lease_until")
            .ok()
            .and_then(layer2_core::seconds_from_rfc3339)
            .ok_or_else(|| domain(super::model::corrupt("prepared lease timestamp is invalid")))?;
        if until > now {
            busy = true;
            continue;
        }
        return Ok(Candidate::Ready(Box::new((candidate, record))));
    }
    Ok(Candidate::Unavailable(if busy {
        ClaimOutcome::Busy
    } else {
        ClaimOutcome::Miss(reason)
    }))
}

fn domain(error: SandboxStoreError) -> AdmissionError {
    AdmissionError::Domain(error)
}
