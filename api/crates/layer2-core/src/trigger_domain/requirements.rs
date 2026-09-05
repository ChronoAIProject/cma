use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{seconds_from_rfc3339, ServiceId};

use super::TriggerError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceRequirementState {
    Satisfied,
    Inactive,
    Missing,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceRequirement {
    pub service: ServiceId,
    pub state: ServiceRequirementState,
}

impl ServiceRequirement {
    pub const fn needs_connection(&self) -> bool {
        !matches!(self.state, ServiceRequirementState::Satisfied)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequirementsSnapshot {
    #[serde(default)]
    pub services: Vec<ServiceRequirement>,
    pub checked_at: String,
}

impl RequirementsSnapshot {
    pub fn validate(&self) -> Result<(), TriggerError> {
        if seconds_from_rfc3339(&self.checked_at).is_none() {
            return Err(TriggerError::new(
                "invalid_timestamp",
                "requirements.checked_at",
            ));
        }
        let mut seen = BTreeSet::new();
        for requirement in &self.services {
            if requirement.service.as_str().trim().is_empty()
                || !seen.insert(requirement.service.clone())
            {
                return Err(TriggerError::new(
                    "invalid_launch_requirements",
                    "requirements.services",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReviewNeed {
    Connection { service: ServiceId },
    TrustConfirmation { commands_digest: String },
    OwnerRequiresReview,
    RepeatChoice,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewRequirements {
    #[serde(default)]
    pub needs: Vec<ReviewNeed>,
    pub can_start_now: bool,
}

impl ReviewRequirements {
    pub fn build(
        requirements: &RequirementsSnapshot,
        trust_digest: Option<String>,
        owner_requires_review: bool,
        repeat_choice: bool,
    ) -> Result<Self, TriggerError> {
        requirements.validate()?;
        let mut needs = requirements
            .services
            .iter()
            .filter(|requirement| requirement.needs_connection())
            .map(|requirement| ReviewNeed::Connection {
                service: requirement.service.clone(),
            })
            .collect::<Vec<_>>();
        if let Some(commands_digest) = trust_digest {
            if !valid_digest(&commands_digest) {
                return Err(TriggerError::new(
                    "invalid_commands_digest",
                    "review.needs.commands_digest",
                ));
            }
            needs.push(ReviewNeed::TrustConfirmation { commands_digest });
        }
        if owner_requires_review {
            needs.push(ReviewNeed::OwnerRequiresReview);
        }
        if repeat_choice {
            needs.push(ReviewNeed::RepeatChoice);
        }
        Ok(Self {
            can_start_now: needs.is_empty(),
            needs,
        })
    }

    pub fn validate(&self) -> Result<(), TriggerError> {
        if self.can_start_now != self.needs.is_empty() {
            return Err(TriggerError::new(
                "invalid_launch_review",
                "review.can_start_now",
            ));
        }
        let mut connections = BTreeSet::new();
        let mut trust = false;
        let mut owner = false;
        let mut choice = false;
        for need in &self.needs {
            let unique = match need {
                ReviewNeed::Connection { service } => {
                    !service.as_str().trim().is_empty() && connections.insert(service.clone())
                }
                ReviewNeed::TrustConfirmation { commands_digest } => {
                    let unique = !trust && valid_digest(commands_digest);
                    trust = true;
                    unique
                }
                ReviewNeed::OwnerRequiresReview => {
                    let unique = !owner;
                    owner = true;
                    unique
                }
                ReviewNeed::RepeatChoice => {
                    let unique = !choice;
                    choice = true;
                    unique
                }
            };
            if !unique {
                return Err(TriggerError::new("invalid_launch_review", "review.needs"));
            }
        }
        Ok(())
    }
}

pub(crate) fn valid_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
