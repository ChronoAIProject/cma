use super::*;
use futures_util::TryStreamExt;

impl MongoResourceExecutionStore {
    pub(super) async fn persist_effect(
        &self,
        intent: &wire::EffectIntentV1,
        session: &mut ClientSession,
    ) -> TxnResult<wire::EffectPermissionV1> {
        let mut member = self.member_in(&intent.registration, session).await?;
        let Some(MemberPermitV1::Execution { ticket }) = &member.permit else {
            return Err(ExecutionError::AuthorityChanged.into());
        };
        intent.validate_for(ticket)?;
        if member.barrier.is_some()
            || member.completion.is_some()
            || !member
                .started
                .as_ref()
                .is_some_and(|start| start.state == wire::NativeStartState::Entered)
        {
            return Err(ExecutionError::AuthorityChanged.into());
        }
        let snapshot = super::super::snapshot::read(&self.sandbox, &member.target, session).await?;
        member.validate_snapshot(&snapshot)?;
        if let Some(family) = member.basis.family() {
            let mut control = self.grant_in(family, member.epoch, session).await?;
            if member.ordinal > control.members {
                return Err(ExecutionError::Corrupt.into());
            }
            control.revision = control.revision.successor()?;
            self.write_grant(&control, session).await?;
        }
        super::super::mutation::fence_lineage(&self.sandbox, &snapshot, session).await?;
        let id = records::effect_id(intent)?;
        if let Some(document) = self
            .collections
            .effects
            .find_one(doc! {"_id":id.as_str()})
            .session(&mut *session)
            .await?
        {
            return Ok(records::read_effect(&document)?.replay_prepared(intent)?);
        }
        if member.effect_count >= MAX_MEMBER_EFFECTS {
            return Err(ExecutionError::Capacity.into());
        }
        member.effect_count += 1;
        let permission = wire::EffectPermissionV1 {
            protocol: 1,
            intent: intent.clone(),
            ledger_sequence: wire::Generation::new(u64::from(member.effect_count))?,
            permission_id: records::hash(&("resource-effect-permission-v1", intent))?,
        };
        let effect = EffectRecordV1 {
            protocol: 1,
            permission: permission.clone(),
            observations: Vec::new(),
        };
        self.collections
            .effects
            .insert_one(records::effect_document(&effect)?)
            .session(&mut *session)
            .await?;
        self.save_member(&member, session).await?;
        Ok(permission)
    }

    pub(super) async fn persist_outcome(
        &self,
        observation: &wire::EffectObservationV1,
        session: &mut ClientSession,
    ) -> TxnResult<()> {
        let registration = registration_id(&wire::MemberKey::Execution {
            execution: observation.execution.clone(),
        })?;
        let id = records::hash(&(
            "resource-effect-identity-v1",
            &registration,
            &observation.effect_id,
        ))?;
        let document = self
            .collections
            .effects
            .find_one(doc! {"_id":id.as_str()})
            .session(&mut *session)
            .await?
            .ok_or(ExecutionError::EvidenceMismatch)?;
        let mut effect = records::read_effect(&document)?;
        effect.observe(observation)?;
        let result = self
            .collections
            .effects
            .replace_one(doc! {"_id":id.as_str()}, records::effect_document(&effect)?)
            .session(session)
            .await?;
        if result.matched_count != 1 {
            return Err(ExecutionError::Corrupt.into());
        }
        Ok(())
    }

    pub(super) async fn effect_page(
        &self,
        registration: &wire::Digest,
        after: u32,
        limit: u32,
    ) -> ExecutionResult<Vec<EffectRecordV1>> {
        if limit == 0 || limit > MAX_PAGE_MEMBERS || after > MAX_MEMBER_EFFECTS {
            return Err(ExecutionError::Capacity);
        }
        let documents: Vec<_> = self
            .collections
            .effects
            .find(doc! {"registration":registration.as_str(),"ordinal":{"$gt":i64::from(after)}})
            .sort(doc! {"ordinal":1})
            .limit(i64::from(limit))
            .await
            .map_err(|_| ExecutionError::Unavailable)?
            .try_collect()
            .await
            .map_err(|_| ExecutionError::Unavailable)?;
        documents.iter().map(records::read_effect).collect()
    }
}
