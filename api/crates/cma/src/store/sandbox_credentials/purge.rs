//! Credential obligations must retain their owner through the purge transaction.
use layer2_core::sandbox::Sandbox;
use mongodb::{
    bson::{doc, Bson, Document},
    ClientSession, Database,
};

use super::{issuance, issuance_record, SANDBOX_CREDENTIALS, SANDBOX_CREDENTIAL_REVOCATIONS};
use crate::sandbox::IssuanceState;

pub(in crate::store) async fn required(
    database: &Database,
    aggregate: &Document,
    sandbox: &Sandbox,
    session: &mut ClientSession,
) -> Result<bool, mongodb::error::Error> {
    if sandbox.credential.current.is_some()
        || sandbox.credential.pending.is_some()
        || !sandbox.credential.revoke_debt.is_empty()
    {
        return Ok(true);
    }
    let marked = match aggregate.get("credential_issuance_fence") {
        None => false,
        Some(Bson::Int64(value)) if *value > 0 => true,
        _ => return Ok(true),
    };
    let intent = database
        .collection::<Document>(issuance::COLLECTION)
        .find_one(doc! {"_id":sandbox.id.as_str()})
        .session(&mut *session)
        .await?;
    match intent {
        None if marked => return Ok(true),
        None => {}
        Some(row) => {
            let Ok(intent) = issuance_record::decode(row.clone()) else {
                return Ok(true);
            };
            if !marked
                || intent.sandbox_id != sandbox.id
                || intent.owner != sandbox.owner
                || !sandbox.credential.source.is_managed()
                || (intent.state != IssuanceState::Accepted
                    && !matches!(
                        issuance::retirement::decode_receipt(&row, &intent),
                        Ok(Some(_))
                    ))
            {
                return Ok(true);
            }
        }
    }
    for collection in [SANDBOX_CREDENTIALS, SANDBOX_CREDENTIAL_REVOCATIONS] {
        if database
            .collection::<Document>(collection)
            .find_one(doc! {"sandbox_id":sandbox.id.as_str()})
            .projection(doc! {"_id":1})
            .session(&mut *session)
            .await?
            .is_some()
        {
            return Ok(true);
        }
    }
    Ok(false)
}
