use super::*;
use crate::store::transaction_terminal::GuardedTerminal as _;

pub(super) async fn start(
    store: &MongoSandboxCredentialStore,
    sandbox: &Sandbox,
) -> Result<ClientSession, CredentialCustodyError> {
    let mut session = store.client.start_session().await.map_err(unavailable)?;
    start_transaction(&mut session).await.map_err(unavailable)?;
    let outcome = async {
        let protocol = sandbox.work.authority_protocol.ok_or_else(corrupt)?;
        let identity = crate::store::work_claim_authority::sandbox_claim(sandbox).map_err(|_| corrupt())?;
        let identity = crate::store::work_publication::sandbox(identity, sandbox.id.as_str()).map_err(|_| corrupt())?;
        let envelope = crate::store::work_schema::sandbox_version(sandbox);
        let protocol = mongodb::bson::serialize_to_bson(&protocol).map_err(|_| corrupt())?;
        let result = store.sandboxes.update_one(doc! {"$and":[identity,{
            "$expr":{"$and":[
                {"$eq":["$document_schema_version",envelope]},
                {"$eq":["$record.work.authority_protocol",{"$literal":protocol}]},
            ]},
            "record.credential":mongodb::bson::serialize_to_bson(&sandbox.credential).map_err(|_| corrupt())?,
            "$or":[{"credential_issuance_fence":{"$exists":false}},
                {"credential_issuance_fence":{"$type":"long","$lt":i64::MAX}}],
        }]},doc! {"$inc":{"credential_issuance_fence":1_i64}})
            .session(&mut session).await.map_err(unavailable)?;
        if result.matched_count != 1 { return Err(corrupt()); }
        Ok(())
    }.await;
    if let Err(error) = outcome {
        if let Err(abort) = session.abort_guarded().await {
            tracing::warn!(error = %abort, "credential authority abort was not acknowledged");
        }
        return Err(error);
    }
    Ok(session)
}

pub(super) async fn finish<T>(
    session: &mut ClientSession,
    outcome: Result<T, CredentialCustodyError>,
) -> Result<T, CredentialCustodyError> {
    let result = match outcome {
        Ok(result) => result,
        Err(error) => {
            if let Err(abort) = session.abort_guarded().await {
                tracing::warn!(error = %abort, "credential custody abort was not acknowledged");
            }
            return Err(error);
        }
    };
    match commit_transaction(session).await {
        CommitOutcome::Committed => Ok(result),
        CommitOutcome::Retry(error)
        | CommitOutcome::Unknown(error)
        | CommitOutcome::Failed(error) => Err(unavailable(error)),
    }
}
