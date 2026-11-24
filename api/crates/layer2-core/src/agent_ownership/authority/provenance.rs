use super::{Authority, Validity};
use crate::agent_ownership::heartbeat_retry::TransientEpisode;
use std::sync::atomic::Ordering;
use tokio::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct HeartbeatMark {
    episode: u64,
    until: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TrustedRetryExpiry(HeartbeatMark);

impl Authority {
    pub(in crate::agent_ownership) fn mark_heartbeat(&self, episode: u64) {
        self.validity.send_modify(|state| {
            state.observe(Instant::now());
            if let Validity::Live {
                until,
                heartbeat_mark,
            } = state
            {
                *heartbeat_mark = Some(HeartbeatMark {
                    episode,
                    until: *until,
                });
            }
        });
    }

    pub(in crate::agent_ownership) fn retire_retry_expiry(
        &self,
        episode: TransientEpisode,
    ) -> bool {
        let mut retired = false;
        self.validity.send_modify(|state| {
            state.observe(Instant::now());
            if let Validity::Expired {
                previous_until,
                mark: Some(mark),
            } = *state
            {
                if episode.matches(mark.episode) && previous_until == mark.until {
                    *state = Validity::RetryExpired {
                        proof: TrustedRetryExpiry(mark),
                    };
                    retired = true;
                }
            }
        });
        if retired {
            self.retiring.store(true, Ordering::Release);
            tracing::info!(parent: None, stage = "owner_renewal", outcome = "trusted_retry_expiry",
                epoch = self.token.epoch, "Expired owner retained for recovery after membership retry");
        }
        retired
    }
}
