//! What a caller may pick from, and where the lists come from.
//!
//! Split from the profile so that file is about what a session is started with
//! and this one about the two outside services that say what may be chosen.
//! Both are ports: this layer states what it needs and never how it is reached,
//! so the code that holds a credential and speaks HTTP lives in the composition
//! root rather than here.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::environment_values::{ServiceId, SkillId};
use crate::ids::SubjectId;

/// A platform service a caller may allow a session to reach.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogService {
    /// Identifier to store in a profile.
    pub id: ServiceId,
    /// Name the owner recognises it by.
    pub name: String,
    /// One-line summary, when the catalogue offers one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A skill a caller may select.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogSkill {
    /// Identifier to store in a profile.
    pub id: SkillId,
    /// Human-readable name.
    pub name: String,
    /// One-line summary.
    pub description: String,
    /// Why this subject can see it, when the catalogue says.
    ///
    /// Free-form and for display only: the core never branches on it, because
    /// the catalogue is the authority on who may select what.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<String>,
}

/// Display metadata resolved for one exact durable skill identifier.
///
/// The key is explicit because profiles may store either a registry GUID or a
/// human-facing slug. Returning only the catalogue row would force consumers to
/// alias it by both forms and could attach an ambiguous slug to the wrong grant.
#[derive(Debug, Clone)]
pub struct DescribedSkill {
    /// The durable identifier the caller asked the catalogue to describe.
    pub requested: SkillId,
    /// The one catalogue row that matched that identifier exactly.
    pub skill: CatalogSkill,
}

/// Failures a catalogue may report.
#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    /// The catalogue could not be reached.
    #[error("skill catalogue unavailable: {0}")]
    Unavailable(String),
    /// The catalogue answered with something unusable.
    #[error("skill catalogue response not usable: {0}")]
    Malformed(String),
}

/// Where the skills a caller may select come from.
///
/// A port, so the core never learns which registry a deployment uses, and a
/// deployment without one still stores and resolves profiles — it simply
/// cannot offer a list to choose from.
#[async_trait::async_trait]
pub trait SkillCatalog: Send + Sync {
    /// Skills this subject may select.
    ///
    /// The credential is the subject's own, when one has been delegated. It is
    /// what lets the catalogue answer with the subject's private and shared
    /// skills rather than only what is public; without it the catalogue answers
    /// as this service rather than as the caller.
    async fn available(
        &self,
        subject: &SubjectId,
        credential: Option<&str>,
    ) -> Result<Vec<CatalogSkill>, CatalogError>;

    /// Describe only the skills a session says it was given.
    ///
    /// Adapters with a bounded, side-effect-free metadata projection should
    /// override this. The default keeps simple catalogues useful and accepts
    /// both durable identifier forms, but returns metadata only when exactly one
    /// catalogue row matches the requested GUID or human-facing slug.
    async fn describe(
        &self,
        subject: &SubjectId,
        credential: Option<&str>,
        skills: &[SkillId],
    ) -> Result<Vec<DescribedSkill>, CatalogError> {
        let available = self.available(subject, credential).await?;
        let mut seen = HashSet::new();
        let mut described = Vec::new();
        for requested in skills.iter().filter(|skill| seen.insert(skill.0.as_str())) {
            let mut exact = available
                .iter()
                .filter(|candidate| candidate.id.0 == requested.0 || candidate.name == requested.0);
            let Some(skill) = exact.next() else {
                continue;
            };
            if exact.next().is_none() {
                described.push(DescribedSkill {
                    requested: requested.clone(),
                    skill: skill.clone(),
                });
            }
        }
        Ok(described)
    }
}

/// Where the services a caller may allow are listed.
///
/// Answered as the caller, always: which services exist is a fact about their
/// account, and this service's own identity would answer with its own. A
/// deployment without one has no picker — profiles still store selections and
/// sessions still honour them, there is simply nothing to choose from.
#[async_trait::async_trait]
pub trait ServiceCatalog: Send + Sync {
    /// Services this subject may allow a session to reach.
    async fn available(
        &self,
        subject: &SubjectId,
        credential: Option<&str>,
    ) -> Result<Vec<CatalogService>, CatalogError>;
}

/// What the service catalogue had to say.
///
/// Three answers, kept apart because only one of them is an error and the
/// other two mean opposite things to whoever is deciding what to do next.
#[derive(Debug, Clone)]
pub enum ServiceAvailability {
    /// This deployment has no catalogue at all.
    ///
    /// **Not an error.** Profiles still store selections and sessions still
    /// honour them; there is simply nothing to choose from. Nothing may be
    /// refused on the strength of it, because a deployment that cannot list
    /// services has not thereby withheld any.
    NoCatalogue,
    /// What this owner has granted. An empty list means they have granted
    /// nothing, which is a real answer and not a missing one.
    Granted(Vec<CatalogService>),
}

impl ServiceAvailability {
    /// The services to offer, which is none in either of the two non-error
    /// cases — the difference between them matters to *authorising*, not to
    /// drawing a picker.
    pub fn listed(self) -> Vec<CatalogService> {
        match self {
            Self::NoCatalogue => Vec::new(),
            Self::Granted(services) => services,
        }
    }

    /// Whether this answer can be used to refuse a selection.
    ///
    /// Only a catalogue that actually answered may refuse anything. A
    /// deployment with no catalogue has withheld nothing, so a session started
    /// against one is provisioned with whatever its profile stored.
    pub fn grants(&self) -> Option<&[CatalogService]> {
        match self {
            Self::NoCatalogue => None,
            Self::Granted(services) => Some(services),
        }
    }
}
