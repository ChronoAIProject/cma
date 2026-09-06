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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceConnectionMode {
    Hosted,
    Manual,
    Unknown,
}

/// Current owner-visible connection guidance, never part of a durable snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceConnectionMetadata {
    pub service: ServiceId,
    pub display_name: Option<String>,
    pub connection_mode: ServiceConnectionMode,
    pub setup_url: Option<String>,
}

impl ServiceConnectionMetadata {
    pub(crate) fn is_valid(&self) -> bool {
        if self.display_name.as_ref().is_some_and(|name| {
            name.trim().is_empty() || name.len() > 256 || name.chars().any(char::is_control)
        }) {
            return false;
        }
        self.setup_url.as_ref().is_none_or(|value| {
            self.connection_mode == ServiceConnectionMode::Manual
                && value.len() <= 2_048
                && url::Url::parse(value).is_ok_and(|url| {
                    url.scheme() == "https"
                        && url.host_str().is_some()
                        && url.username().is_empty()
                        && url.password().is_none()
                        && url.query().is_none()
                        && url.fragment().is_none()
                })
        })
    }
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
    pub fn trust_digest(&self) -> Option<&str> {
        self.needs.iter().find_map(|need| match need {
            ReviewNeed::TrustConfirmation { commands_digest } => Some(commands_digest.as_str()),
            _ => None,
        })
    }

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
        let mut phase = 0;
        for need in &self.needs {
            let (rank, unique) = match need {
                ReviewNeed::Connection { service } => (
                    0,
                    !service.as_str().trim().is_empty() && connections.insert(service.clone()),
                ),
                ReviewNeed::TrustConfirmation { commands_digest } => {
                    let unique = !trust && valid_digest(commands_digest);
                    trust = true;
                    (1, unique)
                }
                ReviewNeed::OwnerRequiresReview => {
                    let unique = !owner;
                    owner = true;
                    (2, unique)
                }
                ReviewNeed::RepeatChoice => {
                    let unique = !choice;
                    choice = true;
                    (3, unique)
                }
            };
            if rank < phase || !unique {
                return Err(TriggerError::new("invalid_launch_review", "review.needs"));
            }
            phase = rank;
        }
        Ok(())
    }

    pub(crate) fn validate_for_launch(
        &self,
        requirements: &RequirementsSnapshot,
        before_start: bool,
    ) -> Result<(), TriggerError> {
        self.validate()?;
        if before_start {
            let expected = requirements
                .services
                .iter()
                .filter(|requirement| requirement.needs_connection())
                .map(|requirement| &requirement.service);
            let actual = self.needs.iter().filter_map(|need| match need {
                ReviewNeed::Connection { service } => Some(service),
                _ => None,
            });
            if !expected.eq(actual) {
                return Err(TriggerError::new("invalid_launch_review", "review.needs"));
            }
        }
        Ok(())
    }
}

pub(crate) fn valid_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
