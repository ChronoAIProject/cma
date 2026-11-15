use mongodb::bson::doc;

use super::{
    store::{document, finish, unavailable},
    validation::{hash, increment, key},
    *,
};

impl AuthorityStore {
    pub(crate) async fn admit_effect(
        &self,
        writer: &WriterIdentity,
        intent: &EffectIntent,
    ) -> Result<EffectAdmission> {
        writer.validate()?;
        intent.validate()?;
        if let Some(existing) = self.effect(&writer.deployment, &intent.effect_id).await? {
            matches_identity(&existing, writer, intent)?;
            return Ok(EffectAdmission::Existing(existing));
        }
        let mut session = self.transaction().await?;
        let result =
            async {
                let previous = self.head_in(&writer.deployment, &mut session).await?;
                let id = key(&writer.deployment, &intent.effect_id)?;
                if let Some(existing) = self.effect_in(&id, &mut session).await? {
                    matches_identity(&existing, writer, intent)?;
                    return Ok(EffectAdmission::Existing(existing));
                }
                // A new attempt ID cannot evade an unresolved identical effect.
                // Concurrent admissions both write the head below, so only one
                // transaction can insert after observing the same empty set.
                if let Some(existing) = self.effects.find_one(doc! {
                "writer.deployment": &writer.deployment,
                "writer.instance_id": &writer.instance_id,
                "writer.config_sha256": &writer.config_sha256,
                "writer.epoch": writer.epoch, "writer.fence": writer.fence,
                "intent.resource_identity_sha256": &intent.resource_identity_sha256,
                "intent.kind": &intent.kind, "intent.request_sha256": &intent.request_sha256,
                "terminal_evidence_sha256": mongodb::bson::Bson::Null,
            }).session(&mut session).await.map_err(unavailable)? {
                let existing: EffectPermit = mongodb::bson::deserialize_from_document(existing)
                    .map_err(|_| AuthorityError::Invalid)?;
                existing.validate()?;
                return Ok(EffectAdmission::Existing(existing));
            }
                if previous.phase != Phase::Active || previous.writer.as_ref() != Some(writer) {
                    return Err(AuthorityError::Closed);
                }
                if previous.active_effects >= 10_000 {
                    return Err(AuthorityError::Closed);
                }
                let mut next = previous.clone();
                next.admission_sequence = increment(previous.admission_sequence)?;
                next.active_effects = increment(previous.active_effects)?;
                let permit = EffectPermit {
                    key: id,
                    schema_version: 1,
                    writer: writer.clone(),
                    intent: intent.clone(),
                    admitted_sequence: next.admission_sequence,
                    revision: 1,
                    observations: Vec::new(),
                    terminal_evidence_sha256: None,
                };
                permit.validate()?;
                // This exact head write conflicts with FREEZING in the same transaction.
                self.write_head(&previous, &next, false, &mut session)
                    .await?;
                self.effects
                    .insert_one(document(&permit)?)
                    .session(&mut session)
                    .await
                    .map_err(unavailable)?;
                Ok(EffectAdmission::New(permit))
            }
            .await;
        finish(&mut session, result).await
    }

    pub(crate) async fn record_effect_outcome(
        &self,
        permit: &EffectPermit,
        outcome: &ObservedOutcome,
    ) -> Result<EffectPermit> {
        permit.validate()?;
        outcome.validate()?;
        let mut current = self
            .effect(&permit.writer.deployment, &permit.intent.effect_id)
            .await?
            .ok_or(AuthorityError::Conflict)?;
        same_permit(&current, permit)?;
        if current.observations.contains(outcome) {
            return Ok(current);
        }
        if current.terminal_evidence_sha256.is_some() || current.observations.len() >= 32 {
            return Err(AuthorityError::Conflict);
        }
        let previous_revision = current.revision;
        current.observations.push(outcome.clone());
        current.revision = increment(current.revision)?;
        let result = self.effects.replace_one(
            doc! {"_id": &current.key, "revision": previous_revision, "terminal_evidence_sha256": mongodb::bson::Bson::Null},
            document(&current)?,
        ).await.map_err(|_| AuthorityError::Uncertain)?;
        if result.matched_count != 1 {
            return Err(AuthorityError::Conflict);
        }
        // Never decrement the head here: a provider ACK is not terminal custody.
        Ok(current)
    }

    pub(crate) async fn settle_effect(
        &self,
        permit: &EffectPermit,
        verifier: &dyn EvidenceVerifier,
    ) -> Result<EffectPermit> {
        permit.validate()?;
        let observed = self
            .effect(&permit.writer.deployment, &permit.intent.effect_id)
            .await?
            .ok_or(AuthorityError::Conflict)?;
        same_permit(&observed, permit)?;
        if observed.terminal_evidence_sha256.is_some() {
            return Ok(observed);
        }
        let head = self
            .load(&permit.writer.deployment)
            .await?
            .ok_or(AuthorityError::Closed)?;
        if head.epoch != permit.writer.epoch
            || head.writer.as_ref() != Some(&permit.writer)
            || head.active_effects < 1
        {
            return Err(AuthorityError::Conflict);
        }
        let evidence = EvidenceRequest {
            deployment: permit.writer.deployment.clone(),
            operation_id: permit.intent.effect_id.clone(),
            epoch: head.epoch,
            control_revision: head.control_revision,
            action: EvidenceAction::SettleEffect,
            request_sha256: hash(&observed)?,
            state_sha256: hash(&head)?,
            subject: EvidenceSubject::Settle {
                permit: Box::new(observed.clone()),
                head: Box::new(head.clone()),
            },
        };
        let evidence = verifier.verify(&evidence).await?.check(&evidence)?;
        let mut settled = observed.clone();
        settled.terminal_evidence_sha256 = Some(evidence);
        settled.revision = increment(observed.revision)?;
        let mut next = head.clone();
        next.active_effects -= 1;
        let mut session = self.transaction().await?;
        let result = async {
            if self.head_in(&head.deployment, &mut session).await? != head
                || self.effect_in(&observed.key, &mut session).await?.as_ref() != Some(&observed)
            {
                return Err(AuthorityError::Conflict);
            }
            self.check_effect_count(&head, &mut session).await?;
            self.write_head(&head, &next, false, &mut session).await?;
            let result = self
                .effects
                .replace_one(
                    doc! {"_id": &observed.key, "revision": observed.revision,
                    "terminal_evidence_sha256": mongodb::bson::Bson::Null},
                    document(&settled)?,
                )
                .session(&mut session)
                .await
                .map_err(unavailable)?;
            if result.matched_count != 1 {
                return Err(AuthorityError::Conflict);
            }
            Ok(settled)
        }
        .await;
        finish(&mut session, result).await
    }
}

fn matches_identity(
    existing: &EffectPermit,
    writer: &WriterIdentity,
    intent: &EffectIntent,
) -> Result<()> {
    if &existing.writer != writer || &existing.intent != intent {
        return Err(AuthorityError::Conflict);
    }
    Ok(())
}

fn same_permit(existing: &EffectPermit, supplied: &EffectPermit) -> Result<()> {
    matches_identity(existing, &supplied.writer, &supplied.intent)?;
    if existing.key != supplied.key || existing.admitted_sequence != supplied.admitted_sequence {
        return Err(AuthorityError::Conflict);
    }
    Ok(())
}
