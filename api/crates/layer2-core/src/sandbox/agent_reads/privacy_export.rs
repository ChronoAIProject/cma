use super::*;
use crate::account_deletion::{AccountStanding, OwnerSessionAuthority};
use crate::resource_access::AuthorizedResource;

use super::super::store::AgentLogStore;
use super::super::{AgentTranscriptPage, AgentTranscriptPageRequest};

const INVENTORY_LIMIT: u16 = 20;

pub struct PrivacyExport {
    reads: Arc<AgentReadService>,
    standing: Arc<dyn AccountStanding>,
    log: Arc<dyn AgentLogStore>,
}

#[derive(Debug, thiserror::Error)]
pub enum PrivacyExportError {
    #[error("verified personal owner session required")]
    OwnerSessionRequired,
    #[error("account is being erased")]
    Erasing,
    #[error("bounded transcript source is unavailable")]
    SourceUnavailable,
    #[error("export source changed")]
    SourceChanged,
    #[error(transparent)]
    Read(#[from] AgentReadError),
}

pub struct ExportInventory {
    pub agents: Page<Agent>,
    held: Vec<AuthorizedResource>,
}

pub struct ExportTranscript {
    pub page: AgentTranscriptPage,
    held: AuthorizedResource,
}

impl PrivacyExport {
    pub fn new(
        reads: Arc<AgentReadService>,
        standing: Arc<dyn AccountStanding>,
        log: Arc<dyn AgentLogStore>,
    ) -> Self {
        Self {
            reads,
            standing,
            log,
        }
    }

    async fn authorize(
        &self,
        caller: &Caller,
        authority: Option<&OwnerSessionAuthority>,
    ) -> Result<(), PrivacyExportError> {
        if authority.is_none_or(|authority| authority.subject() != &caller.subject) {
            return Err(PrivacyExportError::OwnerSessionRequired);
        }
        self.check_standing(caller).await
    }

    async fn check_standing(&self, caller: &Caller) -> Result<(), PrivacyExportError> {
        match self.standing.authoritative(&caller.subject).await {
            Ok(false) => Ok(()),
            Ok(true) => Err(PrivacyExportError::Erasing),
            Err(_) => Err(PrivacyExportError::SourceUnavailable),
        }
    }

    pub async fn inventory(
        &self,
        caller: &Caller,
        authority: Option<&OwnerSessionAuthority>,
        cursor: Option<String>,
    ) -> Result<ExportInventory, PrivacyExportError> {
        self.authorize(caller, authority).await?;
        let request = PageRequest::new(INVENTORY_LIMIT, cursor).map_err(store_error)?;
        let agents = self
            .reads
            .list(caller, &AgentListQuery::default(), &request)
            .await?;
        let mut held = Vec::with_capacity(agents.items.len());
        for agent in &agents.items {
            let guard = self
                .hold(caller, &agent.id, ResourceCapability::ViewDetails)
                .await?;
            if guard.snapshot.lineage.agent.as_ref() != Some(agent) {
                return Err(PrivacyExportError::SourceChanged);
            }
            held.push(guard);
        }
        Ok(ExportInventory { agents, held })
    }

    pub async fn finish_inventory(
        &self,
        caller: &Caller,
        inventory: ExportInventory,
    ) -> Result<(), PrivacyExportError> {
        self.check_standing(caller).await?;
        let access = self.access()?;
        if access
            .personal_agent_fences(&inventory.held, &[])
            .await
            .map_err(authorized::access_error)?
            .is_none()
        {
            for (agent, guard) in inventory.agents.items.iter().zip(&inventory.held) {
                self.verify(guard, agent, ResourceCapability::ViewDetails)
                    .await?;
            }
        }
        self.check_standing(caller).await
    }

    pub async fn transcript(
        &self,
        caller: &Caller,
        authority: Option<&OwnerSessionAuthority>,
        id: &AgentId,
        request: &AgentTranscriptPageRequest,
    ) -> Result<ExportTranscript, PrivacyExportError> {
        self.authorize(caller, authority).await?;
        request.validate()?;
        let held = self
            .hold(caller, id, ResourceCapability::ViewHistory)
            .await?;
        let agent = captured_agent(&held)?;
        require_granted(agent, crate::RuntimeCapability::Conversation)?;
        if request
            .through
            .is_some_and(|through| through != agent.conversation.last_seq)
        {
            return Err(PrivacyExportError::SourceChanged);
        }
        let mut page = self
            .log
            .agent_log_export_page(agent, request)
            .await
            .map_err(|error| match error {
                SandboxStoreError::SequenceConflict => PrivacyExportError::SourceChanged,
                SandboxStoreError::AgentLogCapacityExceeded => {
                    PrivacyExportError::SourceUnavailable
                }
                error => store_error(error).into(),
            })?
            .ok_or(PrivacyExportError::SourceUnavailable)?;
        if page.transcript.agent_id != *id
            || page.transcript.through_seq != agent.conversation.last_seq
        {
            return Err(PrivacyExportError::SourceChanged);
        }
        visibility::shared_transcript(&mut page.transcript);
        Ok(ExportTranscript { page, held })
    }

    pub async fn finish_transcript(
        &self,
        caller: &Caller,
        reading: ExportTranscript,
    ) -> Result<(), PrivacyExportError> {
        let agent = captured_agent(&reading.held)?;
        self.verify(&reading.held, agent, ResourceCapability::ViewHistory)
            .await?;
        self.check_standing(caller).await
    }

    fn access(&self) -> Result<&ResourceAccessService, PrivacyExportError> {
        self.reads
            .access
            .as_deref()
            .ok_or(PrivacyExportError::SourceUnavailable)
    }

    async fn hold(
        &self,
        caller: &Caller,
        id: &AgentId,
        capability: ResourceCapability,
    ) -> Result<AuthorizedResource, PrivacyExportError> {
        self.access()?
            .authorize_personal(
                caller,
                &ResourceTarget::Agent {
                    agent_id: id.clone(),
                },
                capability,
            )
            .await
            .map_err(|error| authorized::access_error(error).into())
    }

    async fn verify(
        &self,
        held: &AuthorizedResource,
        agent: &Agent,
        capability: ResourceCapability,
    ) -> Result<(), PrivacyExportError> {
        let current = self
            .access()?
            .current_read_snapshot(
                held,
                &ResourceTarget::Agent {
                    agent_id: agent.id.clone(),
                },
                capability,
            )
            .await
            .map_err(authorized::access_error)?;
        if current.lineage.agent.as_ref() != Some(agent) {
            return Err(PrivacyExportError::SourceChanged);
        }
        Ok(())
    }
}

fn captured_agent(held: &AuthorizedResource) -> Result<&Agent, PrivacyExportError> {
    held.snapshot
        .lineage
        .agent
        .as_ref()
        .ok_or(PrivacyExportError::SourceChanged)
}
