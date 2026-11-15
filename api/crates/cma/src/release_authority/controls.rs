use mongodb::bson::{DateTime, doc};

use super::{
    store::{document, finish, unavailable},
    validation::{hash, key},
    *,
};

impl AuthorityStore {
    pub(crate) async fn bootstrap(
        &self,
        request: &BootstrapRequest,
        verifier: &dyn EvidenceVerifier,
    ) -> Result<ReleaseOperation> {
        let original = OperationRequest::Bootstrap(request.clone());
        original.validate()?;
        let digest = hash(&original)?;
        if let Some(existing) = self
            .operation(&request.deployment, &request.operation_id)
            .await?
        {
            return replay(existing, &digest);
        }
        if self.load(&request.deployment).await?.is_some() {
            return Err(AuthorityError::Conflict);
        }
        let evidence = EvidenceRequest {
            deployment: request.deployment.clone(),
            operation_id: request.operation_id.clone(),
            epoch: 0,
            control_revision: 0,
            action: EvidenceAction::Bootstrap,
            request_sha256: digest.clone(),
            state_sha256: hash(&Option::<ReleaseHead>::None)?,
            subject: EvidenceSubject::Bootstrap {
                request: request.clone(),
            },
        };
        let evidence = verifier.verify(&evidence).await?.check(&evidence)?;
        let head = ReleaseHead {
            deployment: request.deployment.clone(),
            schema_version: 1,
            epoch: 1,
            control_revision: 1,
            admission_sequence: 0,
            active_effects: 0,
            phase: Phase::Active,
            pair: request.pair.clone(),
            writer: Some(request.writer.clone()),
            writer_fence: 1,
            operation_id: None,
            protected_pairs: vec![request.pair.clone(), request.fallback_pair.clone()],
            committed_at: DateTime::from_millis(0),
        };
        head.validate()?;
        let operation = new_operation(original, &head, digest, evidence, None)?;
        let mut session = self.transaction().await?;
        let result = async {
            // This count is additional consistency, never legacy-effect proof.
            self.check_effect_count(&head, &mut session).await?;
            self.heads
                .insert_one(document(&head)?)
                .session(&mut session)
                .await
                .map_err(unavailable)?;
            self.heads
                .update_one(
                    doc! {"_id": &head.deployment, "control_revision": 1_i64},
                    doc! {"$currentDate": {"committed_at": true}},
                )
                .session(&mut session)
                .await
                .map_err(unavailable)?;
            self.operations
                .insert_one(document(&operation)?)
                .session(&mut session)
                .await
                .map_err(unavailable)?;
            Ok(operation)
        }
        .await;
        finish(&mut session, result).await
    }

    pub(crate) async fn begin(
        &self,
        request: &TransitionRequest,
        verifier: &dyn EvidenceVerifier,
    ) -> Result<ReleaseOperation> {
        let original = OperationRequest::Transition(Box::new(request.clone()));
        original.validate()?;
        let digest = hash(&original)?;
        if let Some(existing) = self
            .operation(&request.deployment, &request.operation_id)
            .await?
        {
            return replay(existing, &digest);
        }
        let observed = self
            .load(&request.deployment)
            .await?
            .ok_or(AuthorityError::Closed)?;
        transitions::begin(&observed, request)?;
        let evidence = EvidenceRequest {
            deployment: request.deployment.clone(),
            operation_id: request.operation_id.clone(),
            epoch: observed.epoch,
            control_revision: observed.control_revision,
            action: EvidenceAction::Begin,
            request_sha256: digest.clone(),
            state_sha256: hash(&observed)?,
            subject: EvidenceSubject::Begin {
                request: Box::new(request.clone()),
                head: Box::new(observed.clone()),
            },
        };
        let evidence = verifier.verify(&evidence).await?.check(&evidence)?;
        let mut session = self.transaction().await?;
        let result = async {
            let current = self.head_in(&request.deployment, &mut session).await?;
            // Admissions may arrive while operator authentication runs. Preserve
            // their counters; freeze must not starve on unrelated admissions.
            let mut control = current.clone();
            control.admission_sequence = observed.admission_sequence;
            control.active_effects = observed.active_effects;
            if control != observed {
                return Err(AuthorityError::Conflict);
            }
            let next = transitions::begin(&current, request)?;
            let operation = new_operation(
                original,
                &next,
                digest,
                evidence,
                current.operation_id.clone(),
            )?;
            self.write_head(&current, &next, false, &mut session)
                .await?;
            self.operations
                .insert_one(document(&operation)?)
                .session(&mut session)
                .await
                .map_err(unavailable)?;
            Ok(operation)
        }
        .await;
        finish(&mut session, result).await
    }

    pub(crate) async fn advance(
        &self,
        request: &AdvanceRequest,
        verifier: &dyn EvidenceVerifier,
    ) -> Result<ControlReceipt> {
        request.operator.validate()?;
        let digest = hash(request)?;
        let mut operation = self
            .operation(&request.deployment, &request.operation_id)
            .await?
            .ok_or(AuthorityError::Conflict)?;
        if let Some(receipt) = operation
            .receipts
            .iter()
            .chain(operation.terminal_recovery_receipt.iter())
            .find(|r| r.request_sha256 == digest)
        {
            return Ok(receipt.clone());
        }
        let observed = self
            .load(&request.deployment)
            .await?
            .ok_or(AuthorityError::Closed)?;
        let next = transitions::advance(&observed, &operation, request)?;
        let evidence = EvidenceRequest {
            deployment: request.deployment.clone(),
            operation_id: request.operation_id.clone(),
            epoch: observed.epoch,
            control_revision: observed.control_revision,
            action: transitions::action(request.action),
            request_sha256: digest.clone(),
            state_sha256: hash(&observed)?,
            subject: EvidenceSubject::Advance {
                request: request.clone(),
                head: Box::new(observed.clone()),
                operation: Box::new(operation.clone()),
            },
        };
        let evidence = verifier.verify(&evidence).await?.check(&evidence)?;
        if next == observed {
            // The same held state returns its existing durable receipt. A new
            // request/revision does not consume an audit slot or fake progress.
            if self.load(&request.deployment).await?.as_ref() != Some(&observed)
                || self
                    .operation(&request.deployment, &request.operation_id)
                    .await?
                    .as_ref()
                    != Some(&operation)
            {
                return Err(AuthorityError::Conflict);
            }
            return operation
                .receipts
                .last()
                .cloned()
                .ok_or(AuthorityError::Invalid);
        }
        let receipt = receipt(&next, digest, evidence);
        let previous_operation = operation.clone();
        if operation.receipts.len() >= 32 {
            operation.terminal_recovery_receipt = Some(receipt.clone());
        } else {
            operation.receipts.push(receipt.clone());
        }
        operation.validate()?;
        let mut session = self.transaction().await?;
        let result = async {
            let current = self.head_in(&request.deployment, &mut session).await?;
            if current != observed
                || self
                    .operation_in(&operation.key, &mut session)
                    .await?
                    .as_ref()
                    != Some(&previous_operation)
            {
                return Err(AuthorityError::Conflict);
            }
            if request.action != AdvanceAction::Hold {
                self.check_effect_count(&current, &mut session).await?;
            }
            self.write_head(
                &current,
                &next,
                request.action == AdvanceAction::Commit,
                &mut session,
            )
            .await?;
            let result = self
                .operations
                .replace_one(
                    doc! {"_id": &operation.key, "request_sha256": &operation.request_sha256},
                    document(&operation)?,
                )
                .session(&mut session)
                .await
                .map_err(unavailable)?;
            if result.matched_count != 1 {
                return Err(AuthorityError::Conflict);
            }
            Ok(receipt)
        }
        .await;
        finish(&mut session, result).await
    }
}

fn replay(existing: ReleaseOperation, expected: &str) -> Result<ReleaseOperation> {
    if existing.request_sha256 != expected {
        return Err(AuthorityError::Conflict);
    }
    Ok(existing)
}

fn receipt(head: &ReleaseHead, request_sha256: String, evidence_sha256: String) -> ControlReceipt {
    ControlReceipt {
        request_sha256,
        evidence_sha256,
        epoch: head.epoch,
        control_revision: head.control_revision,
        phase: head.phase,
    }
}

fn new_operation(
    request: OperationRequest,
    head: &ReleaseHead,
    digest: String,
    evidence: String,
    predecessor_operation_id: Option<String>,
) -> Result<ReleaseOperation> {
    let (deployment, id) = request.identity();
    let operation = ReleaseOperation {
        key: key(deployment, id)?,
        schema_version: 1,
        deployment: deployment.into(),
        operation_id: id.into(),
        request_sha256: digest.clone(),
        request,
        predecessor_operation_id,
        receipts: vec![receipt(head, digest, evidence)],
        terminal_recovery_receipt: None,
    };
    operation.validate()?;
    Ok(operation)
}
