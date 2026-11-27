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
                .settle_validation_in_transaction(identity, refusal, &mut session)
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
        refusal: ValidationRefusal,
        session: &mut ClientSession,
    ) -> Result<(), StartError> {
        let refusal = mongodb::bson::serialize_to_document(&refusal).map_err(unavailable)?;
        let details = |record: &str, generation: &str| {
            // Keep the existing v1 proof row readable by older writers. The
            // companion row has the same owner erasure boundary as the proof.
            doc! {
                "_id": format!("validation:{record}"),
                "document_schema_version": VERSION,
                "owner": identity.owner.as_str(),
                "identity": identity.digest(),
                "generation": generation,
                "problem": refusal.clone(),
            }
        };
        match self.begin_transaction(identity, session).await? {
            Begin::Acquired(fence) => {
                if !publication::refuse_validation(&self.records, identity.owner, &fence, session)
                    .await?
                {
                    return Err(StatusCode::SERVICE_UNAVAILABLE.into());
                }
                self.records
                    .insert_one(details(&fence.record_id, &fence.generation))
                    .session(session)
                    .await?;
            }
            Begin::Existing { .. } => {
                let row = Self::read_record(&self.records, identity, session)
                    .await?
                    .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
                match row.state {
                    Evidence::Pending => return Err(StatusCode::SERVICE_UNAVAILABLE.into()),
                    Evidence::Refused {
                        code: RefusalCode::CreationFailed,
                    } if row.operation_id.is_none() => {}
                    _ => return Err(StatusCode::CONFLICT.into()),
                }
                let expected = details(&row.record_id, &row.generation);
                let saved = self
                    .records
                    .find_one(doc! { "_id": expected.get_str("_id").map_err(unavailable)? })
                    .max_time(IO_BUDGET)
                    .session(session)
                    .await?;
                if saved.as_ref() != Some(&expected) {
                    return Err(StatusCode::CONFLICT.into());
                }
            }
            Begin::Closed => return Err(StatusCode::CONFLICT.into()),
        }
        Ok(())
    }
}
