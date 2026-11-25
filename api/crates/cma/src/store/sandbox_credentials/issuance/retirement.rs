//! Retained revoke-only custody for an unresolved initial mint.
use super::*;
use layer2_core::rfc3339_at;
use std::time::SystemTime;

/// Freeze the exact unresolved issuance under the retirement claim. The
/// original Unknown state continues to guard revocation, home release and purge.
pub(in crate::store::sandbox_credentials) async fn retain_retirement(
    store: &MongoSandboxCredentialStore,
    sandbox: &Sandbox,
    now: SystemTime,
) -> Result<(), CredentialCustodyError> {
    if !crate::sandbox::initial_retirement_issuance_candidate(sandbox) {
        return Ok(());
    }
    let Some(intent) = read(store, sandbox).await? else {
        return Ok(());
    };
    if intent.sandbox_id != sandbox.id || intent.owner != sandbox.owner {
        return Err(corrupt());
    }
    if intent.state != IssuanceState::Unknown
        || intent.generation != 1
        || !matches!(intent.identity, IssuanceIdentity::Prepared { .. })
    {
        return Ok(());
    }
    let mut session = transactions::start(store, sandbox).await?;
    let outcome = async {
        let filter = doc! {"_id": sandbox.id.as_str()};
        let row = store
            .intents
            .find_one(filter.clone())
            .session(&mut session)
            .await
            .map_err(unavailable)?
            .ok_or_else(corrupt)?;
        if decode(row.clone())? != intent {
            return Err(corrupt());
        }
        let exact = row.get_document("record").map_err(|_| corrupt())?.clone();
        let mut frozen = exact.clone();
        frozen.remove("next_check_at");
        if let Some(retirement) = row.get("retirement_custody") {
            let retirement = retirement.as_document().ok_or_else(corrupt)?;
            if retirement.get_i64("version").ok() != Some(1)
                || retirement.get_str("state").ok() != Some("pending")
                || retirement.get_document("issuance").ok() != Some(&frozen)
                || retirement.get_str("reason").ok() != Some("initial_issuance_unresolved")
            {
                return Err(corrupt());
            }
            return Ok(());
        }
        let custody_filter = doc! {"sandbox_id": sandbox.id.as_str()};
        if store
            .collection
            .find_one(custody_filter.clone())
            .session(&mut session)
            .await
            .map_err(unavailable)?
            .is_some()
            || store
                .revocations
                .find_one(custody_filter)
                .session(&mut session)
                .await
                .map_err(unavailable)?
                .is_some()
        {
            return Ok(());
        }
        let updated = store
            .intents
            .update_one(
                doc! {"_id": sandbox.id.as_str(), "record":exact.clone(),
                "retirement_custody":{"$exists":false}},
                doc! {"$set":{"retirement_custody":{
                    "version":1_i64, "state":"pending", "reason":"initial_issuance_unresolved",
                    "issuance":frozen, "recorded_at":rfc3339_at(now),
                }}},
            )
            .session(&mut session)
            .await
            .map_err(unavailable)?;
        if updated.matched_count != 1 {
            return Err(corrupt());
        }
        Ok(())
    }
    .await;
    transactions::finish(&mut session, outcome).await
}

/// A provider result that returns after the retirement handoff may add only
/// revoke-only authority. The immutable marker replaces the stale issuance
/// claim as the receipt's destination, never as delivery authority.
pub(in crate::store::sandbox_credentials) async fn retain_late_result(
    store: &MongoSandboxCredentialStore,
    sandbox: &Sandbox,
    intent: &CredentialIssuance,
    material: &CredentialMaterial,
) -> Result<bool, CredentialCustodyError> {
    validate(intent)?;
    super::super::validate_material(material).map_err(custody_store_error)?;
    let original = sandbox.work.lease.as_ref().ok_or_else(corrupt)?;
    if intent.state != IssuanceState::Unknown
        || intent.generation != 1
        || !matches!(intent.identity, IssuanceIdentity::Prepared { .. })
        || original.holder != intent.original_holder
        || original.fence != intent.original_fence
        || sandbox.id != intent.sandbox_id
        || sandbox.owner != intent.owner
        || material.sandbox_id != intent.sandbox_id
        || material.owner != intent.owner
        || material.authority.generation != intent.generation
        || material.authority.issued_at != intent.created_at
        || !material.source.is_managed()
        || !material.matches_connection(sandbox)
    {
        return Err(corrupt());
    }
    let mut frozen = mongodb::bson::serialize_to_document(intent).map_err(|_| corrupt())?;
    frozen.remove("next_check_at");
    let authority = mongodb::bson::serialize_to_bson(&material.authority).map_err(|_| corrupt())?;
    let mut exact = doc! {
        "_id": intent.sandbox_id.as_str(),
        "document_schema_version": 1_i64,
        "retirement_custody.version": 1_i64,
        "retirement_custody.state": "pending",
        "retirement_custody.reason": "initial_issuance_unresolved",
        "retirement_custody.issuance": &frozen,
    };
    for (field, value) in frozen {
        exact.insert(format!("record.{field}"), value);
    }
    let mut write_filter = exact.clone();
    write_filter.insert(
        "$or",
        vec![
            doc! {"retirement_custody.late_provider_authority":{"$exists":false}},
            doc! {"retirement_custody.late_provider_authority":&authority},
        ],
    );
    let updated = store
        .intents
        .update_one(
            write_filter,
            doc! {"$set":{"retirement_custody.late_provider_authority":&authority}},
        )
        .await
        .map_err(unavailable)?;
    if updated.matched_count == 1 {
        return Ok(true);
    }
    // Absence of the marker supplies no provider finality. A conflicting
    // authoritative receipt cannot replace the frozen responsibility.
    if store
        .intents
        .find_one(exact)
        .await
        .map_err(unavailable)?
        .is_some()
    {
        return Err(corrupt());
    }
    Ok(false)
}
