use super::*;

impl MongoSandboxStore {
    pub(crate) async fn request_agent_access(
        &self,
        requester: AgentAccessRequester,
        mut proposal: AgentAccessProposal,
    ) -> Result<AgentAccessRecord, AgentAccessError> {
        proposal.validate()?;
        proposal.services.sort();
        let id = records::request_id(&requester, &proposal.request_key)?;
        let body_sha256 = records::digest(&proposal)?;
        let now = layer2_core::now_rfc3339();
        let record = AgentAccessRecord {
            scope_mode: Default::default(),
            expected_scope_revision: None,
            id,
            requester,
            proposal,
            body_sha256,
            phase: AgentAccessPhase::Pending,
            decision: None,
            lease: None,
            progress: AgentAccessProgress::default(),
            approved_at: None,
            retry_at: 0,
            created_at: now.clone(),
            updated_at: now,
            revision: 1,
            error: None,
        };
        let AgentAccessRecord {
            id,
            requester,
            proposal,
            body_sha256,
            ..
        } = &record;
        transaction!(self, session, {
            let snapshot =
                super::lineage::validate_lineage_transaction(self, requester, session, true)
                    .await?;
            if let Some(value) = self
                .agent_access_requests
                .find_one(doc! {"_id": id})
                .session(&mut *session)
                .await?
            {
                let record = read(&value)?;
                if &record.body_sha256 != body_sha256
                    || record.requester.creation != requester.creation
                    || record.requester.conversation != requester.conversation
                {
                    return Err(AgentAccessError::Conflict.into());
                }
                return Ok(record);
            }
            super::retry::validate(self, requester, proposal, id, session).await?;
            if self
                .agent_access_requests
                .count_documents(
                    doc! {"record.requester.agent_id": requester.agent_id.as_str(),
                    "record.phase":{"$in":ACTIVE}},
                )
                .session(&mut *session)
                .await?
                >= 4
            {
                return Err(AgentAccessError::Conflict.into());
            }
            fence(self, &snapshot, session).await?;
            self.agent_access_requests
                .insert_one(document(&record)?)
                .session(session)
                .await?;
            Ok(record.clone())
        })
    }

    pub(crate) async fn agent_access_request(
        &self,
        id: &str,
    ) -> Result<Option<AgentAccessRecord>, AgentAccessError> {
        self.agent_access_requests
            .find_one(doc! {"_id":id})
            .await
            .map_err(|_| AgentAccessError::Unavailable)?
            .as_ref()
            .map(read)
            .transpose()
    }

    pub(crate) async fn list_agent_access_requests(
        &self,
        owner: &SubjectId,
        agent: &AgentId,
    ) -> Result<Vec<AgentAccessRecord>, AgentAccessError> {
        transaction!(self, session, {
            let snapshot = crate::store::agent_provenance::read_snapshot(self, agent, session)
                .await
                .map_err(provenance_error)?;
            if snapshot
                .resource
                .lineage
                .agent
                .as_ref()
                .is_none_or(|value| value.owner != *owner)
            {
                return Err(AgentAccessError::NotFound.into());
            }
            let mut records = vec![];
            // Keep outstanding consent visible even after a long request history.
            for phases in [doc! {"$in":ACTIVE}, doc! {"$nin":ACTIVE}] {
                let mut cursor = self
                    .agent_access_requests
                    .find(doc! {"record.requester.agent_id":agent.as_str(),
                    "record.requester.owner":owner.as_str(),"record.phase":phases})
                    .sort(doc! {"record.created_at":-1,"_id":1})
                    .limit(100)
                    .session(&mut *session)
                    .await?;
                while cursor.advance(&mut *session).await? {
                    records.push(read(&cursor.deserialize_current()?)?);
                }
            }
            Ok(records)
        })
    }
}
