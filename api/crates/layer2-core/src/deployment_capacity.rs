//! The declared reservation ceiling for this deployment.

use serde::{Deserialize, Serialize};

use crate::size::SessionSize;

/// How much this deployment says it may reserve, in total.
///
/// These figures are declared, never observed. Runtime supply is shared, so
/// this service cannot turn its own records into a claim about all space in use.
/// The check is therefore an early refusal for work this deployment can account
/// for, not proof that a later runtime request must succeed.
///
/// `None` on one dimension means no ceiling was declared for that dimension and
/// it is not checked. Absence is deliberately different from zero: settings
/// written before this field existed must continue to admit sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DeploymentCapacity {
    /// Total processor reservation this deployment may promise, in millicores.
    pub cpu_millis: Option<u64>,
    /// Total working-memory reservation this deployment may promise, in MiB.
    pub memory_mib: Option<u64>,
    /// What an explicitly unsized session costs this ledger.
    ///
    /// The runtime defaults are opaque configuration text and are never parsed
    /// in this layer. When this is absent and an unsized session must be counted,
    /// no number is invented and the check is left inert.
    pub assumed_session_size: Option<SessionSize>,
}

impl DeploymentCapacity {
    /// Whether at least one dimension has a declared ceiling.
    #[must_use]
    pub fn is_declared(self) -> bool {
        self.cpu_millis.is_some() || self.memory_mib.is_some()
    }

    /// Decide whether `requested` fits beside the reservations already promised.
    ///
    /// `in_flight` must already have had any unsized sessions priced with the
    /// administrator's declared assumption. A caller that cannot do that must
    /// skip this check rather than pass an incomplete sum.
    pub fn check(
        self,
        in_flight: Reserved,
        requested: SessionSize,
    ) -> Result<CapacityRoom, CapacityShortfall> {
        debug_assert_eq!(
            in_flight.unsized_sessions, 0,
            "an incomplete reservation sum must not be checked"
        );

        let requested_cpu_millis = u64::from(requested.cpu_millis());
        let requested_memory_mib = u64::from(requested.memory_mib());
        let free_cpu_millis = self
            .cpu_millis
            .map(|declared| declared.saturating_sub(in_flight.cpu_millis))
            // An undeclared dimension imposes no shortfall. The requested
            // figure is the finite amount this decision can promise is enough.
            .unwrap_or(requested_cpu_millis);
        let free_memory_mib = self
            .memory_mib
            .map(|declared| declared.saturating_sub(in_flight.memory_mib))
            .unwrap_or(requested_memory_mib);

        let cpu_short = self.cpu_millis.is_some() && requested_cpu_millis > free_cpu_millis;
        let memory_short = self.memory_mib.is_some() && requested_memory_mib > free_memory_mib;
        if cpu_short || memory_short {
            return Err(CapacityShortfall {
                requested_cpu_millis,
                requested_memory_mib,
                free_cpu_millis,
                free_memory_mib,
            });
        }

        Ok(CapacityRoom {
            free_cpu_millis,
            free_memory_mib,
        })
    }
}

/// What this deployment has already promised to sessions that want a runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Reserved {
    /// Sessions represented in this sum.
    pub sessions: u64,
    /// Sessions whose record did not name a numeric size.
    pub unsized_sessions: u64,
    /// Processor reservations from records that named a size, in millicores.
    pub cpu_millis: u64,
    /// Working-memory reservations from records that named a size, in MiB.
    pub memory_mib: u64,
}

impl Reserved {
    /// Add one durable session to this sum without inventing an absent size.
    pub fn include(&mut self, size: Option<SessionSize>) {
        self.sessions = self.sessions.saturating_add(1);
        match size {
            Some(size) => {
                self.cpu_millis = self.cpu_millis.saturating_add(u64::from(size.cpu_millis()));
                self.memory_mib = self.memory_mib.saturating_add(u64::from(size.memory_mib()));
            }
            None => self.unsized_sessions = self.unsized_sessions.saturating_add(1),
        }
    }

    /// Price every unsized session with the administrator's declared assumption.
    #[must_use]
    pub fn assuming(mut self, size: SessionSize) -> Self {
        self.cpu_millis = self.cpu_millis.saturating_add(
            self.unsized_sessions
                .saturating_mul(u64::from(size.cpu_millis())),
        );
        self.memory_mib = self.memory_mib.saturating_add(
            self.unsized_sessions
                .saturating_mul(u64::from(size.memory_mib())),
        );
        self.unsized_sessions = 0;
        self
    }
}

/// The declared headroom before admitting a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapacityRoom {
    pub free_cpu_millis: u64,
    pub free_memory_mib: u64,
}

/// A request larger than at least one declared dimension's remaining room.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapacityShortfall {
    pub requested_cpu_millis: u64,
    pub requested_memory_mib: u64,
    pub free_cpu_millis: u64,
    pub free_memory_mib: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size(cpu_millis: u32, memory_mib: u32) -> SessionSize {
        SessionSize::new(cpu_millis, memory_mib).expect("test size is non-zero")
    }

    fn declared(cpu_millis: Option<u64>, memory_mib: Option<u64>) -> DeploymentCapacity {
        DeploymentCapacity {
            cpu_millis,
            memory_mib,
            assumed_session_size: None,
        }
    }

    #[test]
    fn an_undeclared_dimension_never_refuses() {
        let result = declared(None, Some(2_048)).check(
            Reserved {
                cpu_millis: u64::MAX,
                memory_mib: 1_024,
                ..Reserved::default()
            },
            size(4_000, 1_024),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn exactly_filling_the_declared_room_is_admitted_and_one_more_is_refused() {
        let capacity = declared(Some(2_000), Some(4_096));
        let reserved = Reserved {
            sessions: 1,
            cpu_millis: 1_000,
            memory_mib: 2_048,
            ..Reserved::default()
        };

        assert!(capacity.check(reserved, size(1_000, 2_048)).is_ok());
        assert_eq!(
            capacity.check(reserved, size(1_001, 2_048)),
            Err(CapacityShortfall {
                requested_cpu_millis: 1_001,
                requested_memory_mib: 2_048,
                free_cpu_millis: 1_000,
                free_memory_mib: 2_048,
            })
        );
    }

    #[test]
    fn a_shortfall_on_either_dimension_refuses_and_names_both() {
        let capacity = declared(Some(2_000), Some(4_096));
        let reserved = Reserved {
            sessions: 1,
            cpu_millis: 1_250,
            memory_mib: 3_072,
            ..Reserved::default()
        };
        for requested in [size(751, 1_024), size(750, 1_025)] {
            assert_eq!(
                capacity.check(reserved, requested),
                Err(CapacityShortfall {
                    requested_cpu_millis: u64::from(requested.cpu_millis()),
                    requested_memory_mib: u64::from(requested.memory_mib()),
                    free_cpu_millis: 750,
                    free_memory_mib: 1_024,
                })
            );
        }
    }

    #[test]
    fn an_assumption_prices_every_unsized_session_and_never_as_zero() {
        let reserved = Reserved {
            sessions: 3,
            unsized_sessions: 2,
            cpu_millis: 500,
            memory_mib: 512,
        }
        .assuming(size(1_000, 1_024));

        assert_eq!(reserved.unsized_sessions, 0);
        assert_eq!(reserved.cpu_millis, 2_500);
        assert_eq!(reserved.memory_mib, 2_560);
    }
}
