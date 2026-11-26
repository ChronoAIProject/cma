use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::SystemTime;

use async_trait::async_trait;

use crate::ids::AgentId;
use crate::seconds_from_rfc3339;

use super::*;
use crate::sandbox::{AgentTitleEffect, AgentTitleEffectIntent, AgentTitleEffectState};
mod custody;
use crate::sandbox::{AgentLogEntry, AgentLogKind};

/// Transactional in-memory reference for the agent-title store contract.
#[derive(Clone, Default)]
pub struct InMemoryAgentTitleStore {
    state: Arc<Mutex<MemoryState>>,
    custody_enabled: bool,
    lose_next_completion_response: Arc<AtomicBool>,
}

#[derive(Default)]
struct MemoryState {
    agents: BTreeMap<AgentId, MemoryAgent>,
}

#[derive(Default)]
struct MemoryAgent {
    logs: Vec<AgentLogEntry>,
    title: Option<StoredAgentTitle>,
    work: TitleWork,
    effect: AgentTitleEffectState,
}

#[derive(Default)]
struct TitleWork {
    attempted: bool,
    attempts: u32,
    next_attempt_at: Option<SystemTime>,
    settled: bool,
    lease: Option<AgentTitleClaim>,
    completed: Option<CompletedClaim>,
}

struct CompletedClaim {
    holder: String,
    token: String,
}

impl InMemoryAgentTitleStore {
    pub fn with_title_effect_custody(mut self, enabled: bool) -> Self {
        self.custody_enabled = enabled;
        self
    }

    pub fn register_agent(&self, id: AgentId) -> Result<(), AgentTitleStoreError> {
        let mut state = self.lock();
        if state.agents.contains_key(&id) {
            return Err(AgentTitleStoreError::Duplicate);
        }
        state.agents.insert(id, MemoryAgent::default());
        Ok(())
    }

    /// Append durable turn evidence with the same gap-free sequence as the real log.
    pub fn append_agent_log(
        &self,
        id: &AgentId,
        appended_at: impl Into<String>,
        kind: AgentLogKind,
    ) -> Result<AgentLogEntry, AgentTitleStoreError> {
        if matches!(kind, AgentLogKind::TitleSet { .. }) {
            return Err(AgentTitleStoreError::Corrupt);
        }
        let mut state = self.lock();
        let agent = state
            .agents
            .get_mut(id)
            .ok_or(AgentTitleStoreError::NotFound)?;
        append_log(agent, id, appended_at.into(), kind)
    }

    /// Arm one committed response loss to prove exact-token completion replay.
    pub fn lose_next_completion_response(&self) {
        self.lose_next_completion_response
            .store(true, Ordering::SeqCst);
    }

    pub fn title_event_count(&self, id: &AgentId) -> Result<usize, AgentTitleStoreError> {
        let state = self.lock();
        let agent = state.agents.get(id).ok_or(AgentTitleStoreError::NotFound)?;
        Ok(agent
            .logs
            .iter()
            .filter(|entry| matches!(entry.kind, AgentLogKind::TitleSet { .. }))
            .count())
    }

    fn lock(&self) -> MutexGuard<'_, MemoryState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[async_trait]
impl AgentTitleStore for InMemoryAgentTitleStore {
    async fn agent_title_effect(
        &self,
        id: &AgentId,
    ) -> Result<AgentTitleEffectState, AgentTitleStoreError> {
        let state = self.lock();
        let agent = state.agents.get(id).ok_or(AgentTitleStoreError::NotFound)?;
        Ok(
            if self.custody_enabled && agent.effect == AgentTitleEffectState::Legacy {
                AgentTitleEffectState::Ready
            } else {
                agent.effect.clone()
            },
        )
    }
    async fn begin_agent_title_effect(
        &self,
        claim: &AgentTitleClaim,
        intent: &AgentTitleEffectIntent,
    ) -> Result<bool, AgentTitleStoreError> {
        self.mutate_effect(claim, intent, None)
    }
    async fn validate_agent_title_effect(
        &self,
        claim: &AgentTitleClaim,
        intent: &AgentTitleEffectIntent,
    ) -> Result<bool, AgentTitleStoreError> {
        self.validate_effect(claim, intent)
    }
    async fn accept_agent_title_effect(
        &self,
        claim: &AgentTitleClaim,
        intent: &AgentTitleEffectIntent,
        title: &AgentTitle,
    ) -> Result<bool, AgentTitleStoreError> {
        self.mutate_effect(claim, intent, Some(title))
    }

    async fn agent_title_candidates(
        &self,
        page: &PageRequest,
    ) -> Result<Page<AgentId>, AgentTitleStoreError> {
        if page.limit == 0 || page.limit > crate::sandbox::store::MAX_PAGE_SIZE {
            return Err(AgentTitleStoreError::InvalidPage);
        }
        if page.cursor.as_ref().is_some_and(String::is_empty) {
            return Err(AgentTitleStoreError::InvalidPage);
        }

        let state = self.lock();
        let mut raw = state
            .agents
            .iter()
            .filter(|(id, _)| {
                page.cursor
                    .as_ref()
                    .is_none_or(|cursor| id.as_str() > cursor.as_str())
            })
            .collect::<Vec<_>>();
        let has_more = raw.len() > usize::from(page.limit);
        raw.truncate(usize::from(page.limit));
        let next = has_more
            .then(|| raw.last().map(|(id, _)| id.to_string()))
            .flatten();
        let items = raw
            .into_iter()
            .filter(|(_, agent)| {
                agent
                    .title
                    .as_ref()
                    .is_none_or(|title| title.source == AgentTitleSource::Fallback)
                    && (!agent.effect.unknown() || agent.title.is_none())
                    && !agent.work.settled
                    && agent
                        .work
                        .next_attempt_at
                        .is_none_or(|at| at <= SystemTime::now())
                    && AgentTitleTranscript::from_complete_log(&agent.logs).is_some()
            })
            .map(|(id, _)| id.clone())
            .collect();
        Ok(Page { items, next })
    }

    async fn agent_title_transcript(
        &self,
        id: &AgentId,
    ) -> Result<Option<AgentTitleTranscript>, AgentTitleStoreError> {
        let state = self.lock();
        let agent = state.agents.get(id).ok_or(AgentTitleStoreError::NotFound)?;
        Ok(AgentTitleTranscript::from_complete_log(&agent.logs))
    }

    async fn acquire_agent_title_claim(
        &self,
        request: &AgentTitleClaimRequest,
        now: SystemTime,
    ) -> Result<Option<AgentTitleClaim>, AgentTitleStoreError> {
        if request.holder().trim().is_empty() || request.holder().len() > 256 {
            return Err(AgentTitleStoreError::InvalidClaim);
        }
        let expires_at = now
            .checked_add(request.ttl())
            .ok_or(AgentTitleStoreError::InvalidClaim)?;
        let mut state = self.lock();
        let Some(agent) = state.agents.get_mut(request.agent_id()) else {
            return Ok(None);
        };
        if agent
            .title
            .as_ref()
            .is_some_and(|title| title.source != AgentTitleSource::Fallback)
            || AgentTitleTranscript::from_complete_log(&agent.logs).is_none()
        {
            return Ok(None);
        }
        if let Some(lease) = agent.work.lease.as_ref() {
            if lease.expires_at() > now {
                let exact = lease.holder() == request.holder() && lease.token() == request.token();
                return Ok(exact.then(|| lease.clone()));
            }
        }

        // An unresolved provider effect is never redispatched. Until its
        // prompt-derived fallback is stored, it may take a fallback-only claim.
        let unknown = agent.effect.unknown();
        if unknown && agent.title.is_some() {
            return Ok(None);
        }
        let accepted = agent.effect.accepted().is_some();
        if !accepted
            && (agent.work.settled || agent.work.next_attempt_at.is_some_and(|at| at > now))
        {
            return Ok(None);
        }
        let mode = if !accepted
            && (unknown || agent.work.attempts >= super::super::AGENT_TITLE_MAX_ATTEMPTS)
        {
            AgentTitleClaimMode::FallbackOnly
        } else {
            agent.work.attempts += u32::from(!accepted);
            AgentTitleClaimMode::Generate
        };
        if self.custody_enabled && agent.effect == AgentTitleEffectState::Legacy {
            agent.effect = if agent.work.attempted {
                AgentTitleEffectState::LegacyUnknown
            } else {
                AgentTitleEffectState::Ready
            };
        }
        agent.work.attempted = true;
        let claim = AgentTitleClaim::acquired(request.clone(), mode, now, expires_at)
            .with_attempts(agent.work.attempts);
        agent.work.next_attempt_at = Some(claim.next_attempt_at());
        agent.work.lease = Some(claim.clone());
        Ok(Some(claim))
    }

    async fn complete_agent_title(
        &self,
        claim: &AgentTitleClaim,
        title: &AgentTitle,
        source: AgentTitleSource,
        set_at: String,
    ) -> Result<bool, AgentTitleStoreError> {
        if seconds_from_rfc3339(&set_at).is_none() || source == AgentTitleSource::User {
            return Err(if source == AgentTitleSource::User {
                AgentTitleStoreError::InvalidClaim
            } else {
                AgentTitleStoreError::InvalidTimestamp
            });
        }
        let mut state = self.lock();
        let Some(agent) = state.agents.get_mut(claim.agent_id()) else {
            return Ok(false);
        };
        if agent
            .title
            .as_ref()
            .is_some_and(|title| title.source != AgentTitleSource::Fallback)
            || agent.work.completed.as_ref().is_some_and(|completed| {
                completed.holder == claim.holder() && completed.token == claim.token()
            })
        {
            let exact = agent.work.completed.as_ref().is_some_and(|completed| {
                completed.holder == claim.holder() && completed.token == claim.token()
            });
            return Ok(exact);
        }
        let exact_lease = agent.work.lease.as_ref().is_some_and(|lease| {
            lease.holder() == claim.holder()
                && lease.token() == claim.token()
                && lease.mode() == claim.mode()
        });
        if !exact_lease
            || !agent.effect.permits_title(title, source)
            || agent.effect.unknown() && claim.mode() != AgentTitleClaimMode::FallbackOnly
            || (agent.effect != AgentTitleEffectState::Legacy
                && claim.expires_at() <= SystemTime::now())
        {
            return Ok(false);
        }
        let active_mode = agent
            .work
            .lease
            .as_ref()
            .expect("the exact lease was just checked")
            .mode();
        if active_mode == AgentTitleClaimMode::FallbackOnly && source != AgentTitleSource::Fallback
        {
            return Ok(false);
        }

        agent.title = Some(StoredAgentTitle {
            agent_id: claim.agent_id().clone(),
            title: title.clone(),
            source,
            set_at: set_at.clone(),
        });
        agent.work.lease = None;
        agent.work.settled = claim.terminal_fallback() || source != AgentTitleSource::Fallback;
        agent.work.completed = Some(CompletedClaim {
            holder: claim.holder().to_owned(),
            token: claim.token().to_owned(),
        });
        append_log(
            agent,
            claim.agent_id(),
            set_at,
            AgentLogKind::TitleSet {
                title: title.as_str().to_owned(),
                source: source.as_str().to_owned(),
            },
        )?;

        if self
            .lose_next_completion_response
            .swap(false, Ordering::SeqCst)
        {
            return Err(AgentTitleStoreError::Unavailable);
        }
        Ok(true)
    }

    async fn set_user_agent_title(
        &self,
        id: &AgentId,
        title: &AgentTitle,
        set_at: String,
    ) -> Result<SetAgentTitleOutcome, AgentTitleStoreError> {
        if seconds_from_rfc3339(&set_at).is_none() {
            return Err(AgentTitleStoreError::InvalidTimestamp);
        }
        let mut state = self.lock();
        let agent = state
            .agents
            .get_mut(id)
            .ok_or(AgentTitleStoreError::NotFound)?;
        if agent.title.is_some() {
            return Ok(SetAgentTitleOutcome::AlreadySet);
        }

        agent.title = Some(StoredAgentTitle {
            agent_id: id.clone(),
            title: title.clone(),
            source: AgentTitleSource::User,
            set_at: set_at.clone(),
        });
        agent.work.lease = None;
        append_log(
            agent,
            id,
            set_at,
            AgentLogKind::TitleSet {
                title: title.as_str().to_owned(),
                source: AgentTitleSource::User.as_str().to_owned(),
            },
        )?;
        Ok(SetAgentTitleOutcome::Set)
    }

    async fn agent_title(
        &self,
        id: &AgentId,
    ) -> Result<Option<StoredAgentTitle>, AgentTitleStoreError> {
        self.lock()
            .agents
            .get(id)
            .map(|agent| agent.title.clone())
            .ok_or(AgentTitleStoreError::NotFound)
    }
}

fn append_log(
    agent: &mut MemoryAgent,
    id: &AgentId,
    appended_at: String,
    kind: AgentLogKind,
) -> Result<AgentLogEntry, AgentTitleStoreError> {
    let seq = u64::try_from(agent.logs.len())
        .map_err(|_| AgentTitleStoreError::SequenceConflict)?
        .checked_add(1)
        .ok_or(AgentTitleStoreError::SequenceConflict)?;
    let entry = AgentLogEntry {
        agent_id: id.clone(),
        seq,
        appended_at,
        kind,
    };
    agent.logs.push(entry.clone());
    Ok(entry)
}
