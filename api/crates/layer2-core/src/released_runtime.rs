//! Durable released-default authority for one deployment, per runtime lane.
//!
//! These are pure transitions. A store applies each one in a single
//! transaction against the exact record it read, so a promotion, a freeze and
//! every first admission of new work are serialized on the lane document.

use serde::{Deserialize, Serialize};

/// The deployment-owned runtime lanes. Custom and Image Manager images are
/// never released defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleasedLane {
    Agent,
    Machine,
}

impl ReleasedLane {
    /// Select from the caller's captured release without falling back across lanes.
    pub fn version_in(self, release: &layer3_adapter::ReleasedRuntime) -> Option<&str> {
        match self {
            Self::Agent => Some(release.agent.as_str()),
            Self::Machine => release.machine.as_deref(),
        }
        .filter(|version| !version.is_empty())
    }
}

/// The image new work in a lane is admitted against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActiveRelease {
    pub image: String,
    /// Advances once per promotion. Admissions fence on it, so candidate
    /// observations never conflict with ordinary new work.
    pub revision: u64,
    pub promoted_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum CandidateState {
    Preparing,
    Ready,
    Refused { reason: String },
}

/// A verified pointer target being prepared; it serves nothing until promoted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateRelease {
    pub image: String,
    pub observation_id: String,
    pub state: CandidateState,
    pub since: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleasedLaneRecord {
    pub lane: ReleasedLane,
    /// Advances on every change to this record.
    pub revision: u64,
    pub active: ActiveRelease,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate: Option<CandidateRelease>,
    pub frozen: bool,
    /// Bumped by every fenced admission so it conflicts with a promotion.
    #[serde(default)]
    pub admissions: u64,
}

/// The exact committed transition, kept so an unknown acknowledgement can be
/// resolved even after a later promotion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleasedTransition {
    pub lane: ReleasedLane,
    pub from_revision: u64,
    pub to_revision: u64,
    pub from_image: String,
    pub to_image: String,
    pub observation_id: String,
    pub at: String,
}

/// Everything a promotion was decided on. The store re-checks each at commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromotionEvidence {
    pub image: String,
    pub observation_id: String,
    pub lane_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReleasedRuntimeRefusal {
    #[error("the lane changed since this decision was made")]
    Stale,
    #[error("the lane is frozen")]
    Frozen,
    #[error("the candidate is not ready for promotion")]
    NotReady,
    #[error("the released runtime record is invalid")]
    Invalid,
}

use ReleasedRuntimeRefusal as Refusal;

impl ReleasedLaneRecord {
    /// The first record of a lane: its verified environment pin is active.
    pub fn bootstrap(lane: ReleasedLane, image: String, at: &str) -> Result<Self, Refusal> {
        let record = Self {
            lane,
            revision: 1,
            active: ActiveRelease {
                image,
                revision: 1,
                promoted_at: at.into(),
            },
            candidate: None,
            frozen: true,
            admissions: 0,
        };
        record.validate()?;
        Ok(record)
    }

    pub fn validate(&self) -> Result<(), Refusal> {
        let pinned = |image: &str| {
            image.rsplit_once("@sha256:").is_some_and(|(name, digest)| {
                !name.is_empty()
                    && digest.len() == 64
                    && digest
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
        };
        let candidate_ok = self.candidate.as_ref().is_none_or(|candidate| {
            pinned(&candidate.image)
                && candidate.image != self.active.image
                && !candidate.observation_id.is_empty()
        });
        if self.revision == 0
            || self.active.revision == 0
            || !pinned(&self.active.image)
            || !candidate_ok
        {
            return Err(Refusal::Invalid);
        }
        Ok(())
    }

    /// Fence the first durable admission of new work against a promotion.
    pub fn admit(&self, image: &str, active_revision: u64) -> Result<Self, Refusal> {
        if self.active.image != image || self.active.revision != active_revision {
            return Err(Refusal::Stale);
        }
        let mut next = self.clone();
        next.admissions = next.admissions.checked_add(1).ok_or(Refusal::Invalid)?;
        Ok(next)
    }

    /// Record a resolved, verified pointer target. The active image clears any
    /// candidate; a new image supersedes the previous candidate.
    pub fn observe(
        &self,
        image: String,
        observation_id: String,
        at: &str,
    ) -> Result<Self, Refusal> {
        if self.frozen {
            return Err(Refusal::Frozen);
        }
        let mut next = self.clone();
        if image == self.active.image {
            next.candidate = None;
        } else if self
            .candidate
            .as_ref()
            .is_some_and(|candidate| candidate.image == image)
        {
            return Ok(self.clone());
        } else {
            next.candidate = Some(CandidateRelease {
                image,
                observation_id,
                state: CandidateState::Preparing,
                since: at.into(),
            });
        }
        if next.candidate == self.candidate {
            return Ok(self.clone());
        }
        next.revision = next.revision.checked_add(1).ok_or(Refusal::Invalid)?;
        next.validate()?;
        Ok(next)
    }

    /// Move the exact observed candidate to `state`; anything else is stale.
    pub fn settle(&self, observation_id: &str, state: CandidateState) -> Result<Self, Refusal> {
        if self.frozen {
            return Err(Refusal::Frozen);
        }
        let Some(candidate) = &self.candidate else {
            return Err(Refusal::Stale);
        };
        if candidate.observation_id != observation_id {
            return Err(Refusal::Stale);
        }
        let mut next = self.clone();
        next.candidate = Some(CandidateRelease {
            state,
            ..candidate.clone()
        });
        next.revision = next.revision.checked_add(1).ok_or(Refusal::Invalid)?;
        Ok(next)
    }

    /// Explicit operator enable after cutover to compatible writers. The
    /// persistence layer still checks the ready writer cohort for each decision.
    pub fn enable_following(&self) -> Result<Self, Refusal> {
        if !self.frozen {
            return Ok(self.clone());
        }
        let mut next = self.clone();
        next.frozen = false;
        next.revision = next.revision.checked_add(1).ok_or(Refusal::Invalid)?;
        Ok(next)
    }

    /// Keep the active release serving while invalidating any decision made by
    /// a resolver before this operator action.
    pub fn freeze(&self) -> Result<Self, Refusal> {
        if self.frozen {
            return Ok(self.clone());
        }
        let mut next = self.clone();
        next.frozen = true;
        next.revision = next.revision.checked_add(1).ok_or(Refusal::Invalid)?;
        Ok(next)
    }

    /// Forget a frozen candidate only after the store confirms its target is settled.
    pub fn clear_frozen_candidate(&self) -> Result<Self, Refusal> {
        if !self.frozen {
            return Err(Refusal::Stale);
        }
        let mut next = self.clone();
        next.candidate = None;
        next.revision = next.revision.checked_add(1).ok_or(Refusal::Invalid)?;
        Ok(next)
    }

    /// Promote the exact ready candidate the evidence names.
    pub fn promote(
        &self,
        evidence: &PromotionEvidence,
        at: &str,
    ) -> Result<(Self, ReleasedTransition), Refusal> {
        if self.frozen {
            return Err(Refusal::Frozen);
        }
        if self.revision != evidence.lane_revision {
            return Err(Refusal::Stale);
        }
        let candidate = self.candidate.as_ref().ok_or(Refusal::NotReady)?;
        if candidate.image != evidence.image || candidate.observation_id != evidence.observation_id
        {
            return Err(Refusal::Stale);
        }
        if candidate.state != CandidateState::Ready {
            return Err(Refusal::NotReady);
        }
        let mut next = self.clone();
        next.active = ActiveRelease {
            image: candidate.image.clone(),
            revision: self
                .active
                .revision
                .checked_add(1)
                .ok_or(Refusal::Invalid)?,
            promoted_at: at.into(),
        };
        next.candidate = None;
        next.revision = next.revision.checked_add(1).ok_or(Refusal::Invalid)?;
        next.validate()?;
        let transition = ReleasedTransition {
            lane: self.lane,
            from_revision: self.active.revision,
            to_revision: next.active.revision,
            from_image: self.active.image.clone(),
            to_image: next.active.image.clone(),
            observation_id: candidate.observation_id.clone(),
            at: at.into(),
        };
        Ok((next, transition))
    }
}

#[cfg(test)]
#[path = "released_runtime_tests.rs"]
mod tests;
