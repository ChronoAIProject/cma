use super::*;

impl AgentReadService {
    // Used inside the combined read so the command result is still private
    // when the caller acquires the final authorization snapshot.
    pub(in crate::sandbox) async fn observed_command_unreleased(
        &self,
        authorization: &AgentEventStreamAuthorization,
        id: &crate::ids::AgentResponseId,
    ) -> Result<Arc<super::super::agent_io::AdmittedAgentCommand>, AgentReadError> {
        if authorization.observing.is_none() {
            return Err(AgentReadError::NotFound);
        }
        let service = self.observation.as_ref().ok_or(AgentReadError::NotFound)?;
        service.command(authorization, id).await
    }
    pub fn shared_observation_enabled(&self) -> bool {
        self.observation.is_some()
    }

    pub fn observe_output_stream(
        &self,
        authorization: &mut AgentEventStreamAuthorization,
    ) -> Result<Option<super::super::agent_observation::AgentObservationSubscription>, AgentReadError>
    {
        // Unrouted compatibility requests retain their established read path.
        if super::super::agent_observation::observation_agent().is_none() {
            return Ok(None);
        }
        self.observe_event_stream(authorization)
    }

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
        if let Some(service) = self
            .observation
            .as_ref()
            .filter(|_| !super::super::agent_observation::durable_read_fallback())
        {
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

impl AgentReadService {
    /// Memoize a pure fold of already verified immutable inputs. The caller's
    /// fingerprint must cover the complete input, view data and fold version.
    /// No credential, grant, deadline or authorization result belongs here.
    /// This uses the same bounded payload pool as source pages, and does not
    /// replace the independent release fence after computing or reusing a fold.
    pub fn memoize_verified_fold<T>(&self, fingerprint: [u8; 32], build: impl FnOnce() -> T) -> T
    where
        T: Clone + serde::Serialize + Send + Sync + 'static,
    {
        match &self.observation {
            Some(service) => service.memoize_verified_fold(fingerprint, build),
            None => build(),
        }
    }
}
