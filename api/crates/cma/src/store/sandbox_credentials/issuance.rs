//! One unresolved issuance barrier per Sandbox, retained across generation changes.
use super::issuance_record::{decode, validate};
use super::*;
use crate::sandbox::{CredentialIssuance, IssuanceIdentity, IssuanceObservation, IssuanceState};
use crate::store::sandbox_mongo::{commit_transaction, start_transaction, CommitOutcome};
use layer2_core::sandbox::Sandbox;
use mongodb::bson::{Bson, Document};
use mongodb::options::ReadConcern;
use mongodb::ClientSession;

pub(in crate::store) const COLLECTION: &str = "sandbox_credential_issuance";
pub(in crate::store) mod legacy;
pub(super) mod retirement;
mod transactions;

fn document(intent: &CredentialIssuance) -> Result<Document, CredentialCustodyError> {
    validate(intent)?;
    Ok(
        doc! {"_id":intent.sandbox_id.as_str(),"document_schema_version":1_i64,
        "record":mongodb::bson::serialize_to_bson(intent).map_err(|_| corrupt())?},
    )
}

pub(super) async fn read(
    store: &MongoSandboxCredentialStore,
    sandbox: &Sandbox,
) -> Result<Option<CredentialIssuance>, CredentialCustodyError> {
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .await
        .map_err(unavailable)?;
    let intent = store
        .intents
        .find_one(doc! {"_id":sandbox.id.as_str()})
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .await
        .map_err(unavailable)?
        .map(decode)
        .transpose()?;
    if intent.is_none()
        && store
            .sandboxes
            .find_one(doc! {
                "_id":sandbox.id.as_str(), "credential_issuance_fence":{"$exists":true}
            })
            .projection(doc! {"_id":1})
            .read_concern(ReadConcern::snapshot())
            .session(&mut session)
            .await
            .map_err(unavailable)?
            .is_some()
    {
        return Err(CredentialCustodyError::Unavailable(
            "credential intent is missing behind a retained issuance marker".into(),
        ));
    }
    Ok(intent)
}

pub(super) async fn begin(
    store: &MongoSandboxCredentialStore,
    sandbox: &Sandbox,
    intent: &CredentialIssuance,
) -> Result<bool, CredentialCustodyError> {
    validate(intent)?;
    let lease = sandbox.work.lease.as_ref().ok_or_else(corrupt)?;
    if !matches!(intent.identity, IssuanceIdentity::Prepared { .. })
        || intent.sandbox_id != sandbox.id
        || intent.owner != sandbox.owner
        || intent.generation != sandbox.credential.next_generation().ok_or_else(corrupt)?
        || intent.original_holder != lease.holder
        || intent.original_fence != lease.fence
        || intent.state != IssuanceState::Unknown
    {
        return Err(corrupt());
    }
    let mut session = transactions::start(store, sandbox).await?;
    let outcome = async {
        let existing = store
            .intents
            .find_one(doc! {"_id":sandbox.id.as_str()})
            .session(&mut session)
            .await
            .map_err(unavailable)?;
        let filter = if let Some(row) = existing {
            let previous = decode(row)?;
            if previous.owner != sandbox.owner
                || previous.state == IssuanceState::Unknown
                || previous.generation > sandbox.credential.key_generation
                || previous.generation >= intent.generation
            {
                return Ok(false);
            }
            doc! {"_id":sandbox.id.as_str(),"record.operation_id":previous.operation_id,
            "record.state":"accepted"}
        } else {
            let aggregate = store
                .sandboxes
                .find_one(doc! {"_id":sandbox.id.as_str()})
                .projection(doc! {"credential_issuance_fence":1})
                .session(&mut session)
                .await
                .map_err(unavailable)?
                .ok_or_else(corrupt)?;
            if aggregate.get_i64("credential_issuance_fence").ok() != Some(1) {
                // The first intent and its retained aggregate marker commit
                // atomically. Missing custody after that boundary is not proof
                // of no dispatch and cannot become another POST.
                return Err(CredentialCustodyError::Unavailable(
                    "credential intent is missing behind a retained issuance marker".into(),
                ));
            }
            doc! {"_id":sandbox.id.as_str(),"record":{"$exists":false}}
        };
        crate::store::work_schema::floor::retain_rows(
            &store.collection.clone_with_type::<Document>(),
            &mut session,
        )
        .await
        .map_err(unavailable)?;
        let result = store
            .intents
            .replace_one(filter, document(intent)?)
            .upsert(true)
            .session(&mut session)
            .await
            .map_err(unavailable)?;
        Ok(result.matched_count == 1 || result.upserted_id.is_some())
    }
    .await;
    transactions::finish(&mut session, outcome).await
}

pub(super) async fn accept(
    store: &MongoSandboxCredentialStore,
    sandbox: &Sandbox,
    intent: &CredentialIssuance,
    material: &CredentialMaterial,
) -> Result<(), CredentialCustodyError> {
    validate(intent)?;
    let original = sandbox.work.lease.as_ref().ok_or_else(corrupt)?;
    if original.holder != intent.original_holder
        || original.fence != intent.original_fence
        || intent.state != IssuanceState::Unknown
        || !matches!(intent.identity, IssuanceIdentity::Prepared { .. })
        || material.sandbox_id != intent.sandbox_id
        || material.owner != intent.owner
        || material.authority.generation != intent.generation
        || !material.source.is_managed()
        || !material.matches_connection(sandbox)
    {
        return Err(corrupt());
    }
    let material = store.document(material).map_err(custody_store_error)?;
    let mut session = transactions::start(store, sandbox).await?;
    let outcome = async {
        let mut accepted = intent.clone();
        accepted.state = IssuanceState::Accepted;
        let result = store
            .intents
            .replace_one(
                doc! {"_id":intent.sandbox_id.as_str(),
                "record":mongodb::bson::serialize_to_bson(intent).map_err(|_| corrupt())?},
                document(&accepted)?,
            )
            .session(&mut session)
            .await
            .map_err(unavailable)?;
        if result.matched_count != 1 {
            return Err(corrupt());
        }
        store
            .collection
            .insert_one(material)
            .session(&mut session)
            .await
            .map_err(unavailable)?;
        Ok(())
    }
    .await;
    transactions::finish(&mut session, outcome).await
}

pub(super) async fn observe(
    store: &MongoSandboxCredentialStore,
    sandbox: &Sandbox,
    intent: &CredentialIssuance,
    observation: IssuanceObservation,
) -> Result<(), CredentialCustodyError> {
    let observation = match observation {
        IssuanceObservation::MetadataOnly { matches } => {
            doc! {"kind":"metadata_only","matches":i64::try_from(matches).map_err(|_| corrupt())?}
        }
        IssuanceObservation::Unavailable => doc! {"kind":"unavailable"},
    };
    let mut session = transactions::start(store, sandbox).await?;
    let outcome = async {
        let result = store.intents.update_one(doc! {"_id":sandbox.id.as_str(),
            "record.operation_id":&intent.operation_id,"record.owner":sandbox.owner.as_str(),
            "record.state":"unknown", "record.next_check_at":{"$regex":crate::store::work_claim_authority::CANONICAL_TIME},
            "$expr":{"$and":[{"$eq":[{"$type":"$record.next_check_at"},"string"]},
                {"$ne":[{"$convert":{"input":"$record.next_check_at","to":"date","onError":Bson::Null}},Bson::Null]},
                {"$lte":[{"$convert":{"input":"$record.next_check_at","to":"date","onError":Bson::Null}},"$$NOW"]}]}},
            vec![doc! {"$set":{"last_observation":{"$literal":observation},"observed_at":"$$NOW",
                "record.next_check_at":{"$dateToString":{"format":"%Y-%m-%dT%H:%M:%SZ","timezone":"UTC",
                    "date":{"$dateAdd":{"startDate":"$$NOW","unit":"second","amount":30_i64}}}}}}])
            .session(&mut session).await.map_err(unavailable)?;
        if result.matched_count != 1 {
            // A concurrent reconciler can move the due time while this request
            // observes the provider. That is unresolved custody, not corruption.
            return Err(CredentialCustodyError::Unavailable(
                "credential observation is no longer due".into()));
        }
        Ok(())
    }.await;
    transactions::finish(&mut session, outcome).await
}

fn corrupt() -> CredentialCustodyError {
    CredentialCustodyError::Corrupt("credential issuance custody refused".into())
}
fn unavailable(_: mongodb::error::Error) -> CredentialCustodyError {
    CredentialCustodyError::Unavailable("credential issuance outcome is unresolved".into())
}
