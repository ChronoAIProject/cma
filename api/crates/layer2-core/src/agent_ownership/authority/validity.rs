use tokio::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Validity {
    Live { until: Instant },
    ReceiverCancelled { previous_until: Instant },
    Expired { previous_until: Instant },
    Revoked,
}

impl Validity {
    pub(super) fn observe(&mut self, now: Instant) {
        if let Self::Live { until } = *self {
            if now >= until {
                *self = Self::Expired {
                    previous_until: until,
                };
            }
        }
    }

    pub(super) fn deadline(self) -> Option<Instant> {
        match self {
            Self::Live { until } => Some(until),
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
