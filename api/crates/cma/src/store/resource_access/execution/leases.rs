use super::*;

impl MongoResourceExecutionStore {
    async fn admission(
        &self,
        authority: &ExecutionAuthority,
        session: &mut ClientSession,
    ) -> TxnResult<Option<GrantControlV1>> {
        authority.validate_live().await?;
        let epoch = self.collections.require_activation(session).await?;
        if epoch != authority.epoch() {
            return Err(ExecutionError::AuthorityChanged.into());
        }
        let snapshot =
            super::super::snapshot::read(&self.sandbox, authority.target(), session).await?;
        authority.validate_current(&snapshot)?;
        let control = match authority.basis().family() {
            Some(family) => Some(self.grant_in(family, epoch, session).await?),
            None => None,
        };
        super::super::mutation::fence_lineage(&self.sandbox, &snapshot, session).await?;
        // Expiry is checked again after storage awaits. This still does not
        // replace the receiver's final deadline/barrier check at native entry.
        authority.validate_current(&snapshot)?;
        authority.validate_live().await?;
        Ok(control)
    }

    async fn touch_authority(
        &self,
        authority: &ExecutionAuthority,
        session: &mut ClientSession,
    ) -> TxnResult<()> {
        if let Some(mut control) = self.admission(authority, session).await? {
            control.revision = control.revision.successor()?;
            self.write_grant(&control, None, session).await?;
        }
        Ok(())
    }

    pub(super) async fn register_member(
        &self,
        authority: &ExecutionAuthority,
        member: &wire::MemberKey,
        session: &mut ClientSession,
    ) -> TxnResult<ExecutionRecordV1> {
        member.validate()?;
        if let wire::MemberKey::Execution { execution } = member {
            if !matches!(authority.target(), layer2_core::resource_access::ResourceTarget::Agent { agent_id }
                if agent_id.as_str() == execution.agent_id.as_str())
            {
                return Err(ExecutionError::AuthorityChanged.into());
            }
        }
        let id = registration_id(member)?;
        let mut control = self.admission(authority, session).await?;
        if let Some(document) = self
            .collections
            .members
            .find_one(doc! {"_id":id.as_str()})
            .session(&mut *session)
            .await?
        {
            let previous = records::read_member(&document)?;
            previous.require_authority(authority)?;
            if previous.member != *member {
                return Err(ExecutionError::IdempotencyConflict.into());
            }
            if let Some(control) = &mut control {
                control.revision = control.revision.successor()?;
                self.write_grant(control, None, session).await?;
            }
            return Ok(previous);
        }
        let ordinal = match &mut control {
            Some(control) => control.reserve_member()?,
            None => 1,
        };
        let record = ExecutionRecordV1 {
            protocol: 1,
            registration: id,
            epoch: authority.epoch(),
            target: authority.target().clone(),
            basis: authority.basis().clone(),
            member: member.clone(),
            ordinal,
            permit: None,
            continuation: None,
            started: None,
            barrier: None,
            completion: None,
            effect_count: 0,
        };
        let mut document = records::member_document(&record)?;
        let family = authority.basis().family();
        let write_keys = self.writes_erasure_keys(family, session).await?;
        if write_keys {
            document.insert(records::ERASURE_KEYS, erasure::admission(authority, true)?);
        }
        self.collections
            .members
            .insert_one(document)
            .session(&mut *session)
            .await?;
        if let Some(control) = control {
            let keys = write_keys
                .then(|| erasure::admission(authority, false))
                .transpose()?;
            self.write_grant(&control, keys, session).await?;
        }
        Ok(record)
    }

    pub(super) async fn persist_ticket(
        &self,
        authority: &ExecutionAuthority,
        ticket: &wire::ExecutionTicketV1,
        session: &mut ClientSession,
    ) -> TxnResult<wire::ExecutionTicketV1> {
        ticket.validate()?;
        self.touch_authority(authority, session).await?;
        let mut member = self.member_in(&ticket.registration, session).await?;
        member.require_authority(authority)?;
        if member.barrier.is_some() || member.completion.is_some() {
            return Err(ExecutionError::AuthorityChanged.into());
        }
        match &member.permit {
            Some(MemberPermitV1::Execution { ticket: original }) if original == ticket => {
                return Ok(original.clone())
            }
            Some(_) => return Err(ExecutionError::IdempotencyConflict.into()),
            None => {}
        }
        member.permit = Some(MemberPermitV1::Execution {
            ticket: ticket.clone(),
        });
        self.save_member(&member, session).await?;
        Ok(ticket.clone())
    }

    pub(super) async fn persist_continuation(
        &self,
        authority: &ExecutionAuthority,
        lease: &wire::ContinuationV1,
        session: &mut ClientSession,
    ) -> TxnResult<wire::ContinuationV1> {
        self.touch_authority(authority, session).await?;
        let mut member = self.member_in(&lease.registration, session).await?;
        member.require_authority(authority)?;
        member.set_continuation(lease)?;
        self.save_member(&member, session).await?;
        Ok(lease.clone())
    }

    pub(super) async fn persist_release(
        &self,
        authority: &ExecutionAuthority,
        permit: &wire::ReleasePermitV1,
        session: &mut ClientSession,
    ) -> TxnResult<wire::ReleasePermitV1> {
        self.touch_authority(authority, session).await?;
        let mut member = self.member_in(&permit.registration, session).await?;
        member.require_authority(authority)?;
        member.set_release(permit)?;
        self.save_member(&member, session).await?;
        Ok(permit.clone())
    }

    pub(super) async fn persist_start(
        &self,
        observation: &wire::StartObservationV1,
        session: &mut ClientSession,
    ) -> TxnResult<()> {
        let mut member = self.member_in(&observation.registration, session).await?;
        member.observe_start(observation)?;
        self.save_member(&member, session).await
    }
}
