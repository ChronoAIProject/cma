//! Exact owner evidence for a replacement live subscription.
use std::{sync::Arc, time::Duration};
use tokio::time::Instant;

use super::{AgentLivePublisher, AgentLiveSubscription, LiveFrame};
use crate::agent_ownership::{Authority, OwnershipCoordinator, OwnershipToken};

#[derive(Clone)]
pub struct AgentLiveInterruption {
    pub owner: OwnershipToken,
    pub deadline: Instant,
}

#[derive(Clone)]
pub struct AgentLiveAttachment {
    authority: Arc<Authority>,
    coordinator: Arc<OwnershipCoordinator>,
}

impl AgentLiveAttachment {
    pub fn owner(&self) -> &OwnershipToken {
        &self.authority.token
    }

    fn succeeds(&self, interruption: &AgentLiveInterruption) -> bool {
        let owner = &self.authority.token;
        owner.agent_id == interruption.owner.agent_id
            && owner.epoch > interruption.owner.epoch
            && owner.protocol == interruption.owner.protocol
            && self.authority.is_current()
            && self.coordinator.is_ready()
    }

    /// Recheck the exact owner and member after the authorized head read.
    /// The read can shorten the usable lifetime but cannot renew authority.
    pub async fn confirm(&self) -> bool {
        if !self.authority.is_current() || !self.coordinator.is_ready() {
            return false;
        }
        let started = Instant::now();
        let Ok(remaining) = self
            .coordinator
            .store
            .live_lease_remaining_ms(&self.authority.token)
            .await
        else {
            return false;
        };
        started
            .checked_add(Duration::from_millis(remaining))
            .is_some_and(|deadline| Instant::now() < deadline)
            && self.authority.is_current()
            && self.coordinator.is_ready()
    }
}

impl AgentLivePublisher {
    pub fn attached(&self, authority: Arc<Authority>, coordinator: Arc<OwnershipCoordinator>) {
        self.attachment.send_replace(Some(AgentLiveAttachment {
            authority,
            coordinator,
        }));
    }

    pub fn interrupted(&self, owner: OwnershipToken, deadline: Instant) {
        let _ = self
            .sender
            .send(LiveFrame::Interrupted(AgentLiveInterruption {
                owner,
                deadline,
            }));
    }
}

impl AgentLiveSubscription {
    pub fn attached_owner(&self) -> Option<OwnershipToken> {
        let attachment = self.attachment.as_ref()?.borrow();
        attachment
            .as_ref()
            .filter(|attachment| {
                attachment.authority.is_current() && attachment.coordinator.is_ready()
            })
            .map(|attachment| attachment.owner().clone())
    }

    pub async fn successor(
        &mut self,
        interruption: &AgentLiveInterruption,
    ) -> Option<AgentLiveAttachment> {
        let observer = self.attachment.as_mut()?;
        let value = observer
            .wait_for(|value| {
                value
                    .as_ref()
                    .is_some_and(|value| value.succeeds(interruption))
            })
            .await
            .ok()?;
        value.clone()
    }
}
