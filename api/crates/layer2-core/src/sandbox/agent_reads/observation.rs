use super::*;

impl AgentReadService {
    pub async fn owner_observation_authorization(
        &self,
        caller: &Caller,
        id: &AgentId,
    ) -> Result<Option<AgentEventStreamAuthorization>, AgentReadError> {
        if self.observation.is_none() {
            return Ok(None);
        }
        let access = self
            .access
            .as_ref()
            .ok_or_else(|| AgentReadError::Unavailable("resource access absent".into()))?;
        let held = access
            .authorize_personal(
                caller,
                &ResourceTarget::Agent {
                    agent_id: id.clone(),
                },
                ResourceCapability::ViewHistory,
            )
            .await
            .map_err(authorized::access_error)?;
        Ok(Some(AgentEventStreamAuthorization::personal(
            held,
            caller.clone(),
            id.clone(),
        )))
    }

    pub(super) async fn visit_observed_page(
        &self,
        agent: &Agent,
        after: u64,
        through: u64,
        limit: u16,
        tail: bool,
        visit: &mut (dyn FnMut(AgentLogEntry) -> Result<(), SandboxStoreError> + Send),
    ) -> Result<(), SandboxStoreError> {
        if let Some(service) = &self.observation {
            let page = service
                .source_page(agent, (after, through, limit, tail), || async {
                    let mut page = Some(Vec::new());
                    let mut bytes = 0;
                    self.visit_source_page(agent, after, through, limit, tail, &mut |entry| {
                        bytes += serde_json::to_vec(&entry)
                            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?
                            .len();
                        if bytes > super::super::agent_observation::MAX_PAGE_BYTES {
                            page = None;
                        }
                        if let Some(page) = &mut page {
                            page.push(entry);
                        }
                        Ok(())
                    })
                    .await?;
                    Ok(page)
                })
                .await?;
            if let Some(entries) = &page.entries {
                for entry in entries.iter() {
                    visit(entry.clone())?;
                }
                return Ok(());
            }
            // Keep the per-Agent load guard through an oversized reread.
            return self
                .visit_source_page(agent, after, through, limit, tail, visit)
                .await;
        }
        // Oversized original payloads stay on the existing separately bounded
        // visitor path. Never turn them into a cache or subscriber queue exception.
        self.visit_source_page(agent, after, through, limit, tail, visit)
            .await
    }

    async fn visit_source_page(
        &self,
        agent: &Agent,
        after: u64,
        through: u64,
        limit: u16,
        tail: bool,
        visit: &mut (dyn FnMut(AgentLogEntry) -> Result<(), SandboxStoreError> + Send),
    ) -> Result<(), SandboxStoreError> {
        if tail {
            self.store
                .visit_agent_log_page_with_tail(&agent.id, after, through, limit, visit)
                .await
        } else {
            self.store
                .visit_agent_log_page_with_predecessor(&agent.id, after, through, limit, visit)
                .await
        }
    }
}
