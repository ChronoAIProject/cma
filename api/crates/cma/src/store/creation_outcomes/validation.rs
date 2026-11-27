use super::*;

impl CreationOutcomes {
    pub(super) async fn settle_validation(
        &self,
        identity: &Identity<'_>,
        refusal: ValidationRefusal,
    ) -> Result<(), StatusCode> {
        if !self.validation_settlement_enabled()
            || identity.operation != Operation::AgentBootstrap
            || identity.deployment != self.config.deployment
        {
            return Err(StatusCode::CONFLICT);
        }
        let period = identity
            .period()
            .map_err(identity_error)?
            .ok_or(StatusCode::BAD_REQUEST)?;
        let refusal = mongodb::bson::serialize_to_document(&refusal).map_err(unavailable)?;
        if period.epoch != self.config.epoch {
            // A retired epoch can prove a previous refusal through its snapshot.
            let (snapshot, mut session) = self.read_snapshot().await?;
            let records = snapshot.collection(COLLECTION);
            return confirm_validation(
                &records,
                identity,
                &refusal,
                &mut session,
                StatusCode::CONFLICT,
            )
            .await;
        }
        for _ in 0..4 {
            let mut session = self
                .database
                .client()
                .start_session()
                .await
                .map_err(unavailable)?;
            transaction::start_transaction(&mut session)
                .await
                .map_err(unavailable)?;
            let result = self
                .settle_validation_in_transaction(identity, &refusal, &mut session)
                .await;
            if let Err(error) = result {
                session.abort_guarded().await.map_err(unavailable)?;
                if matches!(&error, StartError::Driver(error) if transaction::is_transient(error)) {
                    continue;
                }
                return Err(error.status());
            }
            match transaction::commit_transaction(&mut session).await {
                transaction::CommitOutcome::Committed => {
                    tracing::debug!(creation_proof = %record_id(identity), "Covered creation validation refusal confirmed");
                    return Ok(());
                }
                transaction::CommitOutcome::Retry(_) => continue,
                transaction::CommitOutcome::Unknown(_) | transaction::CommitOutcome::Failed(_) => {
                    return Err(StatusCode::SERVICE_UNAVAILABLE);
                }
            }
        }
        Err(StatusCode::SERVICE_UNAVAILABLE)
    }

    async fn settle_validation_in_transaction(
        &self,
        identity: &Identity<'_>,
        refusal: &Document,
        session: &mut ClientSession,
    ) -> Result<(), StartError> {
        match self.begin_transaction(identity, session).await? {
            Begin::Acquired(fence) => {
                if !publication::refuse_validation(&self.records, identity.owner, &fence, session)
                    .await?
                {
                    return Err(StatusCode::SERVICE_UNAVAILABLE.into());
                }
                self.records
                    .insert_one(validation_details(
                        identity,
                        refusal,
                        &fence.record_id,
                        &fence.generation,
                    ))
                    .session(session)
                    .await?;
            }
            Begin::Existing { .. } => {
                confirm_validation(
                    &self.records,
                    identity,
                    refusal,
                    session,
                    StatusCode::SERVICE_UNAVAILABLE,
                )
                .await?;
            }
            Begin::Closed => return Err(StatusCode::CONFLICT.into()),
        }
        Ok(())
    }
}

fn validation_details(
    identity: &Identity<'_>,
    refusal: &Document,
    record: &str,
    generation: &str,
) -> Document {
    // Keep the existing v1 proof row readable by older writers. The companion
    // row has the same owner erasure boundary as the proof.
    doc! {
        "_id": format!("validation:{record}"),
        "document_schema_version": VERSION,
        "owner": identity.owner.as_str(),
        "identity": identity.digest(),
        "generation": generation,
        "problem": refusal.clone(),
    }
}

async fn confirm_validation(
    records: &Collection<Document>,
    identity: &Identity<'_>,
    refusal: &Document,
    session: &mut ClientSession,
    unsettled: StatusCode,
) -> Result<(), StatusCode> {
    let row = CreationOutcomes::read_record(records, identity, session)
        .await?
        .ok_or(unsettled)?;
    match row.state {
        Evidence::Pending => return Err(unsettled),
        Evidence::Refused {
            code: RefusalCode::CreationFailed,
        } if row.operation_id.is_none() => {}
        _ => return Err(StatusCode::CONFLICT),
    }
    let expected = validation_details(identity, refusal, &row.record_id, &row.generation);
    let saved = records
        .find_one(doc! { "_id": expected.get_str("_id").map_err(unavailable)?, "owner": identity.owner.as_str() })
        .max_time(IO_BUDGET)
        .session(session)
        .await
        .map_err(unavailable)?;
    if saved.as_ref() != Some(&expected) {
        return Err(StatusCode::CONFLICT);
    }
    Ok(())
}
