//! The deployment's released default runtime images for new work.
//!
//! An operation that admits new work takes one [`ReleasedRuntime`] snapshot
//! and uses it for every decision it makes, so a Sandbox is never provisioned
//! from one release while its record, pool or update target names another.
//! Work admitted earlier keeps the target it recorded.

/// One consistent view of both lanes' released default images.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleasedRuntime {
    /// Advances whenever either lane's released image changes.
    pub revision: u64,
    /// The pinned agent-lane image (`repository@sha256:…`).
    pub agent: String,
    /// The pinned machine-lane image, when this deployment provides one.
    pub machine: Option<String>,
}

/// The release a caller captured is no longer current. Retry the admission
/// against a fresh snapshot; nothing was admitted against the stale one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the released runtime changed; retry against the current release")]
pub struct StaleReleasedRuntime;

/// The authority for new work's default images.
pub trait ReleasedRuntimeSource: Send + Sync + std::fmt::Debug {
    /// The current release. Take it once per admitted operation.
    fn snapshot(&self) -> ReleasedRuntime;

    /// Confirm a captured snapshot is still the release before admitting work
    /// against it. A replica that missed a change refuses here and retries.
    fn require_current(&self, released: &ReleasedRuntime) -> Result<(), StaleReleasedRuntime>;
}

/// The deployment's configured pins. They never change for the process.
#[derive(Debug, Clone)]
pub struct PinnedReleasedRuntime(ReleasedRuntime);

impl PinnedReleasedRuntime {
    pub fn new(agent: impl Into<String>, machine: Option<String>) -> Self {
        Self(ReleasedRuntime {
            revision: 0,
            agent: agent.into(),
            machine,
        })
    }
}

impl ReleasedRuntimeSource for PinnedReleasedRuntime {
    fn snapshot(&self) -> ReleasedRuntime {
        self.0.clone()
    }

    fn require_current(&self, released: &ReleasedRuntime) -> Result<(), StaleReleasedRuntime> {
        if *released == self.0 {
            Ok(())
        } else {
            Err(StaleReleasedRuntime)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_release_is_stable_and_refuses_any_other_snapshot() {
        let pinned = PinnedReleasedRuntime::new("repo@sha256:a", Some("machine@sha256:m".into()));
        let first = pinned.snapshot();
        assert_eq!(first, pinned.snapshot());
        assert_eq!(first.revision, 0);
        assert_eq!(pinned.require_current(&first), Ok(()));
        for stale in [
            ReleasedRuntime {
                revision: 1,
                ..first.clone()
            },
            ReleasedRuntime {
                agent: "repo@sha256:b".into(),
                ..first.clone()
            },
            ReleasedRuntime {
                machine: None,
                ..first.clone()
            },
        ] {
            assert_eq!(pinned.require_current(&stale), Err(StaleReleasedRuntime));
        }
    }
}
