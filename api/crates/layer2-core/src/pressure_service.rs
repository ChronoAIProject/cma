//! Recording the durable edges produced by a live pressure fold.

use crate::event::EventKind;
use crate::ids::SessionId;
use crate::pressure::{
    CpuPressureSample, CpuPressureTransition, PressureSample, PressureTransition,
};
use crate::service::SessionService;

impl SessionService {
    /// Fold one reading into a session's independent resource-pressure states.
    ///
    /// There is no caller and nothing to retry. A reading that produces no
    /// edge is ordinary; a durable write failure is already reported by
    /// `flush`.
    pub(crate) async fn note_resource_pressure(
        &self,
        id: &SessionId,
        memory: Option<PressureSample>,
        cpu: Option<CpuPressureSample>,
    ) {
        if memory.is_none() && cpu.is_none() {
            return;
        }
        let settings = self.settings.for_provisioning().await;
        let wrote = {
            let mut entries = self.entries.write().await;
            let Some(entry) = entries.get_mut(id) else {
                return;
            };
            let interactive = entry.session.status.is_interactive();
            let mut wrote = false;
            if let Some(sample) = memory {
                let transition = if interactive {
                    entry
                        .memory_pressure
                        .evaluate(&sample, &settings.memory_pressure)
                } else {
                    entry.memory_pressure.abandon(sample.at)
                };
                if let Some(transition) = transition {
                    crate::log::append(entry, memory_event_from(transition));
                    wrote = true;
                }
            }
            if let Some(sample) = cpu {
                let transition = if interactive {
                    entry.cpu_pressure.evaluate(&sample, &settings.cpu_pressure)
                } else {
                    entry.cpu_pressure.abandon(sample.at)
                };
                if let Some(transition) = transition {
                    crate::log::append(entry, cpu_event_from(transition));
                    wrote = true;
                }
            }
            wrote
        };
        if wrote {
            self.flush(id).await;
        }
    }
}

fn memory_event_from(transition: PressureTransition) -> EventKind {
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

fn cpu_event_from(transition: CpuPressureTransition) -> EventKind {
    match transition {
        CpuPressureTransition::Opened {
            used_millis,
            limit_millis,
            occupancy_permille,
        } => EventKind::CpuPressureOpened {
            used_millis,
            limit_millis,
            occupancy_permille,
        },
        CpuPressureTransition::Closed {
            peak_millis,
            limit_millis,
            peak_permille,
            duration_seconds,
        } => EventKind::CpuPressureClosed {
            peak_millis,
            limit_millis,
            peak_permille,
            duration_seconds,
        },
    }
}
