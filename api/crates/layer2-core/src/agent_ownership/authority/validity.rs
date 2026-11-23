use super::provenance::{HeartbeatMark, TrustedRetryExpiry};
use tokio::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Validity {
    Live {
        until: Instant,
        heartbeat_mark: Option<HeartbeatMark>,
    },
    ReceiverCancelled {
        previous_until: Instant,
    },
    Expired {
        previous_until: Instant,
        mark: Option<HeartbeatMark>,
    },
    Retiring {
        intent: crate::agent_ownership::RetirementIntent,
    },
    RetryExpired {
        proof: TrustedRetryExpiry,
    },
    Unpublished,
    Revoked,
}

impl Validity {
    pub(super) fn observe(&mut self, now: Instant) {
        if let Self::Live {
            until,
            heartbeat_mark,
        } = *self
        {
            if now >= until {
                *self = Self::Expired {
                    previous_until: until,
                    mark: heartbeat_mark,
                };
            }
        }
    }

    pub(super) fn deadline(self) -> Option<Instant> {
        match self {
            Self::Live { until, .. } => Some(until),
            _ => None,
        }
    }

    pub(super) fn revoke(&mut self, now: Instant) {
        self.observe(now);
        if matches!(self, Self::Live { .. }) {
            *self = Self::Revoked;
        }
    }
}
