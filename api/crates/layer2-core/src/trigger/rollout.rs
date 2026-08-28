use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::{Launch, TriggerValidationError};
use crate::SubjectId;

/// Deployment-owned admission and emergency controls for GitHub Triggers.
///
/// Empty allowlists deny admission. That makes enabling the environment flag
/// alone insufficient to expose a publisher or repository accidentally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggerRolloutPolicy {
    pub environment_enabled: bool,
    pub orchestration_paused: bool,
    pub capability_kill_switch: bool,
    pub publisher_subject_ids: BTreeSet<String>,
    pub repository_ids: BTreeSet<u64>,
    pub user_cohort_percent: u8,
    unrestricted: bool,
}

impl TriggerRolloutPolicy {
    pub fn new(
        environment_enabled: bool,
        orchestration_paused: bool,
        capability_kill_switch: bool,
        publisher_subject_ids: impl IntoIterator<Item = String>,
        repository_ids: impl IntoIterator<Item = u64>,
        user_cohort_percent: u8,
    ) -> Result<Self, TriggerValidationError> {
        if user_cohort_percent > 100 {
            return Err(TriggerValidationError::field(
                "trigger_rollout.user_cohort_percent",
                "must be between 0 and 100",
            ));
        }
        let publisher_subject_ids = publisher_subject_ids
            .into_iter()
            .map(|subject| subject.trim().to_string())
            .collect::<BTreeSet<_>>();
        if publisher_subject_ids.iter().any(String::is_empty) {
            return Err(TriggerValidationError::field(
                "trigger_rollout.publisher_subject_ids",
                "contains an empty subject identifier",
            ));
        }
        let repository_ids = repository_ids.into_iter().collect::<BTreeSet<_>>();
        if repository_ids.contains(&0) {
            return Err(TriggerValidationError::field(
                "trigger_rollout.repository_ids",
                "contains the invalid numeric repository identity 0",
            ));
        }
        Ok(Self {
            environment_enabled,
            orchestration_paused,
            capability_kill_switch,
            publisher_subject_ids,
            repository_ids,
            user_cohort_percent,
            unrestricted: false,
        })
    }

    /// Permissive policy used only by existing constructors and unit fixtures.
    /// Production constructs an explicit policy from deployment configuration.
    pub fn allow_all() -> Self {
        Self {
            environment_enabled: true,
            orchestration_paused: false,
            capability_kill_switch: false,
            publisher_subject_ids: BTreeSet::new(),
            repository_ids: BTreeSet::new(),
            user_cohort_percent: 100,
            unrestricted: true,
        }
    }

    pub fn publisher_admission(
        &self,
        publisher: &SubjectId,
        repository_id: u64,
    ) -> Result<(), TriggerRolloutDenial> {
        self.environment()?;
        if !self.unrestricted && !self.publisher_subject_ids.contains(publisher.as_str()) {
            return Err(TriggerRolloutDenial::PublisherNotAllowlisted);
        }
        if !self.unrestricted && !self.repository_ids.contains(&repository_id) {
            return Err(TriggerRolloutDenial::RepositoryNotAllowlisted);
        }
        Ok(())
    }

    pub fn launch_admission(
        &self,
        user: &SubjectId,
        publisher: &SubjectId,
        repository_id: u64,
    ) -> Result<(), TriggerRolloutDenial> {
        self.publisher_admission(publisher, repository_id)?;
        if self.orchestration_paused {
            return Err(TriggerRolloutDenial::OrchestrationPaused);
        }
        if cohort_bucket(user) >= self.user_cohort_percent {
            return Err(TriggerRolloutDenial::UserOutsideCohort);
        }
        Ok(())
    }

    pub fn launch_orchestration(&self, launch: &Launch) -> Result<(), TriggerRolloutDenial> {
        self.launch_admission(
            &launch.owner,
            &launch.revision.content.publisher_subject_id,
            launch.revision.content.upstream.repository.repository_id,
        )
    }

    /// Whether subsequent GitHub capability use must converge to revocation.
    /// Session continuity is deliberately independent of this answer.
    pub fn capability_must_revoke(&self) -> bool {
        self.capability_kill_switch
    }

    fn environment(&self) -> Result<(), TriggerRolloutDenial> {
        if self.environment_enabled {
            Ok(())
        } else {
            Err(TriggerRolloutDenial::EnvironmentDisabled)
        }
    }
}

impl Default for TriggerRolloutPolicy {
    fn default() -> Self {
        Self::allow_all()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum TriggerRolloutDenial {
    #[error("GitHub Triggers are disabled in this environment")]
    EnvironmentDisabled,
    #[error("GitHub Trigger orchestration is paused")]
    OrchestrationPaused,
    #[error("this publisher is outside the deployment rollout")]
    PublisherNotAllowlisted,
    #[error("this numeric repository identity is outside the deployment rollout")]
    RepositoryNotAllowlisted,
    #[error("this user is outside the deterministic rollout cohort")]
    UserOutsideCohort,
}

impl TriggerRolloutDenial {
    pub fn code(self) -> &'static str {
        match self {
            Self::EnvironmentDisabled => "environment_disabled",
            Self::OrchestrationPaused => "orchestration_paused",
            Self::PublisherNotAllowlisted => "publisher_not_allowlisted",
            Self::RepositoryNotAllowlisted => "repository_not_allowlisted",
            Self::UserOutsideCohort => "user_outside_cohort",
        }
    }
}

/// Stable across processes and deployments; no mutable assignment table is
/// needed for a cohort decision that must survive a Recreate rollout.
pub fn cohort_bucket(subject: &SubjectId) -> u8 {
    let digest = Sha256::digest(format!("cma-trigger-rollout-v1\0{}", subject.as_str()));
    let value = u64::from_be_bytes(digest[..8].try_into().expect("SHA-256 has eight bytes"));
    (value % 100) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trigger::tests::revision_fixture;

    #[test]
    fn cohort_assignment_is_stable_and_bounded() {
        let subject = SubjectId("contributor-a".into());
        assert_eq!(cohort_bucket(&subject), cohort_bucket(&subject));
        assert!(cohort_bucket(&subject) < 100);
    }

    #[test]
    fn explicit_allowlists_and_cohort_gate_admission() {
        let publisher = SubjectId("publisher".into());
        let user = SubjectId("contributor".into());
        let repository = 123;
        let policy =
            TriggerRolloutPolicy::new(true, false, false, [publisher.0.clone()], [repository], 100)
                .unwrap();
        assert!(policy
            .launch_admission(&user, &publisher, repository)
            .is_ok());
        assert_eq!(
            policy.launch_admission(&user, &publisher, repository + 1),
            Err(TriggerRolloutDenial::RepositoryNotAllowlisted)
        );
    }

    #[test]
    fn pause_and_kill_switch_are_independent_and_do_not_reclassify_the_session() {
        let mut launch = Launch::new(
            SubjectId("contributor".into()),
            revision_fixture(SubjectId("publisher".into())),
            "2026-08-27T00:00:00Z",
            "rollout-test",
        )
        .unwrap();
        let repository = launch.revision.content.upstream.repository.repository_id;
        let policy =
            TriggerRolloutPolicy::new(true, true, false, ["publisher".into()], [repository], 100)
                .unwrap();
        assert_eq!(
            policy.launch_orchestration(&launch),
            Err(TriggerRolloutDenial::OrchestrationPaused)
        );
        assert!(!policy.capability_must_revoke());
        let kill_switch =
            TriggerRolloutPolicy::new(true, false, true, ["publisher".into()], [repository], 100)
                .unwrap();
        assert!(kill_switch.launch_orchestration(&launch).is_ok());
        assert!(kill_switch.capability_must_revoke());
        assert_eq!(launch.state, super::super::LaunchState::Draft);
        launch
            .block(
                super::super::LaunchBlockedReason::FeaturePaused,
                "2026-08-27T00:00:01Z",
                "rollout-test",
            )
            .unwrap();
        assert_eq!(launch.state, super::super::LaunchState::Draft);
    }
}
