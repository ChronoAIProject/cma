use super::*;
use crate::auth::keys::CredentialRetirementReceipt;
use layer2_core::sandbox::SandboxStatus;

const RECEIPT: &str = "retirement_receipt";

fn matches(intent: &CredentialIssuance, receipt: &CredentialRetirementReceipt) -> bool {
    intent.state == IssuanceState::Unknown
        && matches!(&intent.identity, IssuanceIdentity::Prepared { request_digest, .. }
            if receipt.matches(&intent.operation_id, &intent.owner, request_digest))
}

pub(in crate::store::sandbox_credentials) fn decode_receipt(
    row: &Document,
    intent: &CredentialIssuance,
) -> Result<Option<CredentialRetirementReceipt>, CredentialCustodyError> {
    row.get(RECEIPT)
        .map(|value| {
            let receipt =
                mongodb::bson::deserialize_from_bson(value.clone()).map_err(|_| corrupt())?;
            matches(intent, &receipt)
                .then_some(receipt)
                .ok_or_else(corrupt)
        })
        .transpose()
}

fn check_scope(
    sandbox: &Sandbox,
    intent: &CredentialIssuance,
) -> Result<(), CredentialCustodyError> {
    if sandbox.status != SandboxStatus::Retiring
        || !sandbox.work.step.is_retirement()
        || !sandbox.credential.source.is_managed()
        || intent.owner != sandbox.owner
        || intent.sandbox_id != sandbox.id
        || intent.state != IssuanceState::Unknown
    {
        return Err(corrupt());
    }
    Ok(())
}

fn exact_intent(intent: &CredentialIssuance) -> Result<Document, CredentialCustodyError> {
    Ok(doc! {
        "_id":intent.sandbox_id.as_str(),
        "document_schema_version":1_i64,
        "record":mongodb::bson::serialize_to_bson(intent).map_err(|_| corrupt())?
    })
}

fn positive_marker() -> Document {
    doc! {"$expr":{"$and":[
        {"$eq":[{"$type":"$credential_issuance_fence"},"long"]},
        {"$gt":["$credential_issuance_fence",0_i64]},
    ]}}
}

pub(in crate::store::sandbox_credentials) async fn has_receipt(
    store: &MongoSandboxCredentialStore,
    sandbox: &Sandbox,
    intent: &CredentialIssuance,
) -> Result<bool, CredentialCustodyError> {
    check_scope(sandbox, intent)?;
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .await
        .map_err(unavailable)?;
    let parent = store
        .sandboxes
        .find_one(doc! {"$and":[positive_marker(),{
            "_id":sandbox.id.as_str(), "owner":sandbox.owner.as_str(),
            "record.owner":sandbox.owner.as_str(), "status":"retiring",
            "record.status":"retiring"
        }]})
        .projection(doc! {"_id":1})
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .await
        .map_err(unavailable)?;
    if parent.is_none() {
        return Err(corrupt());
    }
    let row = store
        .intents
        .find_one(exact_intent(intent)?)
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .await
        .map_err(unavailable)?
        .ok_or_else(|| CredentialCustodyError::Unavailable("credential issuance changed".into()))?;
    Ok(decode_receipt(&row, intent)?.is_some())
}

pub(in crate::store::sandbox_credentials) async fn record(
    store: &MongoSandboxCredentialStore,
    sandbox: &Sandbox,
    intent: &CredentialIssuance,
    receipt: &CredentialRetirementReceipt,
) -> Result<(), CredentialCustodyError> {
    check_scope(sandbox, intent)?;
    if !matches(intent, receipt) {
        return Err(corrupt());
    }
    let filter = exact_intent(intent)?;
    let bson = mongodb::bson::serialize_to_bson(receipt).map_err(|_| corrupt())?;
    let mut session = transactions::start_with_marker(
        store,
        sandbox,
        doc! {"$and":[positive_marker(),{"credential_issuance_fence":{"$lt":i64::MAX}}]},
    )
    .await?;
    let outcome = async {
        let row = store
            .intents
            .find_one(filter.clone())
            .session(&mut session)
            .await
            .map_err(unavailable)?
            .ok_or_else(corrupt)?;
        if let Some(existing) = decode_receipt(&row, intent)? {
            return (existing == *receipt).then_some(()).ok_or_else(corrupt);
        }
        let saved = store
            .intents
            .update_one(
                doc! {"$and":[filter,{RECEIPT:{"$exists":false}}]},
                doc! {"$set":{RECEIPT:bson}},
            )
            .session(&mut session)
            .await
            .map_err(unavailable)?;
        (saved.matched_count == 1).then_some(()).ok_or_else(corrupt)
    }
    .await;
    transactions::finish(&mut session, outcome).await
}
