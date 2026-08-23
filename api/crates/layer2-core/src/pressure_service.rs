//! Recording the durable edges produced by a live pressure fold.

use crate::event::EventKind;
use crate::ids::SessionId;
use crate::pressure::{PressureSample, PressureTransition};
use crate::service::SessionService;

impl SessionService {
    /// Fold one reading into a session's memory-pressure state.
    ///
    /// There is no caller and nothing to retry. A reading that produces no
    /// edge is ordinary; a durable write failure is already reported by
    /// `flush`.
    pub(crate) async fn note_memory_pressure(&self, id: &SessionId, sample: PressureSample) {
        let policy = self.settings.for_provisioning().await.memory_pressure;
        {
            let mut entries = self.entries.write().await;
            let Some(entry) = entries.get_mut(id) else {
                return;
            };
            let transition = if entry.session.status.is_interactive() {
                entry.memory_pressure.evaluate(&sample, &policy)
            } else {
                entry.memory_pressure.abandon(sample.at)
            };
            let Some(transition) = transition else {
                return;
            };
            crate::log::append(entry, event_from(transition));
        }
        self.flush(id).await;
    }
}

fn event_from(transition: PressureTransition) -> EventKind {
    match transition {
        PressureTransition::Opened {
            used_bytes,
            limit_bytes,
            occupancy_permille,
        } => EventKind::MemoryPressureOpened {
            used_bytes,
            limit_bytes,
            occupancy_permille,
        },
        PressureTransition::Closed {
            peak_bytes,
            limit_bytes,
            peak_permille,
            duration_seconds,
        } => EventKind::MemoryPressureClosed {
            peak_bytes,
            limit_bytes,
            peak_permille,
            duration_seconds,
        },
    }
}
