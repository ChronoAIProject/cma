//! All owners contend on one transaction fence before consuming global capacity.
use super::*;

mod launches;

impl MongoSandboxStore {
    /// Zero leaves operator policy as the only retained-home ceiling. Enable a
    /// deployment hard cap only after every writer implements the shared fence.
    pub(crate) fn with_global_sandbox_limit(mut self, limit: u16) -> Self {
        self.global_sandbox_limit = limit;
        self
    }
}

pub(super) async fn admit(
    store: &MongoSandboxStore,
    admission: &SandboxAdmission,
    ceilings: &Result<layer2_core::admission::AdmissionCeilings, ()>,
    session: &mut mongodb::ClientSession,
) -> Result<(), AdmissionError> {
    if !matches!(admission.sandbox.origin, SandboxOrigin::Trigger { .. })
        && matches!(ceilings, Ok(policy) if policy.effective_retained_homes(store.global_sandbox_limit).is_none())
    {
        return Ok(());
    }
    let active = super::super::registration_capacity::activation::read(
        &store.sandbox_admissions,
        Some(&mut *session),
    )
    .await
    .map_err(|error| match error {
        super::super::registration_capacity::activation::Error::Driver(error) => {
            AdmissionError::Driver(error)
        }
        super::super::registration_capacity::activation::Error::Unreadable(_) => {
            AdmissionError::Domain(SandboxStoreError::GlobalCapacityReached)
        }
    })?;
    if !active {
        return legacy_admit(store, session).await;
    }
    let reservation = launches::read(store, admission, session).await?;
    let fresh = admission.admission_cause() == layer2_core::admission::AdmissionCause::Fresh;
    let lifecycle_home = matches!(&admission.sandbox.origin, SandboxOrigin::Lifecycle { .. });
    let bound_home =
        admission.binding.is_some() && matches!(&admission.sandbox.origin, SandboxOrigin::Manual);
    let operator = if fresh || lifecycle_home {
        // The operation was admitted, but this insert consumes its first home.
        // Keep Stop continuation authority and require readable capacity here.
        ceilings
            .as_ref()
            .map_err(|_| AdmissionError::Domain(SandboxStoreError::GlobalCapacityReached))?
            .clone()
    } else if bound_home || reservation.checks_operator() {
        // A bound home or replacement grows capacity under readable numeric
        // policy. Unreadable policy cannot strand an admitted continuation.
        ceilings.clone().unwrap_or_default()
    } else {
        // Existing Launches retain the deployment hard cap from #1672 while
        // their resource steps continue under the original admission.
        layer2_core::admission::AdmissionCeilings::default()
    };
    let limit = operator.effective_retained_homes(store.global_sandbox_limit);
    let mut limit = match limit {
        Some(limit) => limit,
        None if matches!(reservation, launches::Reservation::Legacy) => return Ok(()),
        None => {
            fence(store, session).await?;
            launches::complete(store, admission, &reservation, session).await?;
            return Ok(());
        }
    };
    let reserve = operator.existing_reserve.retained_homes.unwrap_or(0);
    if reserve > 0
        && admission.binding.is_none()
        && super::super::registration_capacity::is_public(
            &store.sandbox_admissions,
            &admission.sandbox.owner,
            Some(session),
        )
        .await
        .map_err(AdmissionError::Driver)?
    {
        limit = limit.saturating_sub(reserve);
    }
    if limit == 0 {
        return Err(AdmissionError::Domain(
            SandboxStoreError::GlobalCapacityReached,
        ));
    }
    if matches!(reservation, launches::Reservation::Reserved(_)) {
        fence(store, session).await?;
        launches::complete(store, admission, &reservation, session).await?;
    }
    // Legacy admitted Launches retain their original deployment-cap behavior.
    // Fresh ordinary admissions and new Launch reservations also account for
    // pending homes, preventing one path from spending the other's slots.
    let pending = if fresh || lifecycle_home || bound_home || reservation.counts_pending() {
        launches::pending(store, session).await?
    } else {
        0
    };
    let available = u64::from(limit).saturating_sub(pending);
    if available == 0 {
        return Err(AdmissionError::Domain(
            SandboxStoreError::GlobalCapacityReached,
        ));
    }
    // Paused, provisioning and retiring rows keep their reservation. A resume
    // therefore cannot grow admitted capacity; only completed retirement frees it.
    // Existing rows are counted directly, so activation needs no counter backfill.
    let live = store
        .sandboxes
        .count_documents(doc! { "retired_at": Bson::Null })
        .hint(mongodb::options::Hint::Name("sandboxes_global_live".into()))
        .limit(available)
        .max_time(Duration::from_secs(3))
        .session(&mut *session)
        .await
        .map_err(AdmissionError::Driver)?;
    if live >= available {
        tracing::info!(
            outcome = "global_capacity_refused",
            "sandbox admission refused"
        );
        return Err(AdmissionError::Domain(
            SandboxStoreError::GlobalCapacityReached,
        ));
    }
    if !matches!(reservation, launches::Reservation::Reserved(_)) {
        // Only candidates that can consume a slot write the shared fence.
        // A competing commit invalidates this snapshot when the fence writes,
        // so accepted counts still serialize while full-capacity refusals do
        // not keep otherwise doomed transactions contending on the hot row.
        fence(store, session).await?;
    }
    if matches!(reservation, launches::Reservation::Reused(_)) {
        launches::complete(store, admission, &reservation, session).await?;
    }
    Ok(())
}

/// Before activation, preserve #1672's fence-before-count worker authority.
async fn legacy_admit(
    store: &MongoSandboxStore,
    session: &mut mongodb::ClientSession,
) -> Result<(), AdmissionError> {
    let limit = store.global_sandbox_limit;
    if limit == 0 {
        return Ok(());
    }
    fence(store, session).await?;
    let live = store
        .sandboxes
        .count_documents(doc! {"retired_at": Bson::Null})
        .hint(mongodb::options::Hint::Name("sandboxes_global_live".into()))
        .limit(u64::from(limit))
        .max_time(Duration::from_secs(3))
        .session(session)
        .await
        .map_err(AdmissionError::Driver)?;
    if live >= u64::from(limit) {
        tracing::info!(
            outcome = "global_capacity_refused",
            "sandbox admission refused"
        );
        return Err(AdmissionError::Domain(
            SandboxStoreError::GlobalCapacityReached,
        ));
    }
    Ok(())
}

async fn fence(
    store: &MongoSandboxStore,
    session: &mut mongodb::ClientSession,
) -> Result<(), AdmissionError> {
    // A structured _id cannot collide with any subject's string _id. The write
    // forces different-owner transactions to retry with a fresh snapshot.
    store.sandbox_admissions.update_one(
        doc! {"_id": {"scope": "global_sandbox_capacity"}},
        doc! {"$inc": {"fence": 1_i64}, "$setOnInsert": {"document_schema_version": DOCUMENT_SCHEMA_VERSION}},
    ).upsert(true).session(session).await.map_err(|error| {
        if is_duplicate_key(&error) {AdmissionError::Retry(error)} else {AdmissionError::Driver(error)}
    })?;
    Ok(())
}
