//! Reusable environment bundles a session can be started with.
//!
//! A profile is the answer to "start me a session that already has these
//! variables set, this software present, and these skills loaded". It is owned
//! by one subject, reused across as many sessions as they like, and edited
//! independently of any session that used it.
//!
//! Two things here are load bearing:
//!
//! * **The baseline cannot be revoked.** Some entries are part of what a
//!   session *is* on this platform rather than something a caller opted into.
//!   They are merged in on write and again on resolve, so a client that omits
//!   them still gets them and a client that deletes them gets them back. The
//!   concrete entries are injected by the deployment — this file owns the rule,
//!   not the list.
//! * **Values are never rendered.** A variable's value is exactly the place a
//!   caller will put a token, so `Debug` redacts it and nothing here logs one.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::ids::{EnvironmentProfileId, KernelId, SubjectId};

// Re-exported so the split is a move rather than a rename: everything that
// referred to these through this module still does, and a reader looking for
// "what a profile is made of" is pointed at the file that holds it.
pub use crate::environment_catalog::{
    CatalogError, CatalogService, CatalogSkill, DescribedSkill, ServiceAvailability,
    ServiceCatalog, SkillCatalog,
};
pub use crate::environment_values::{
    EnvironmentVariable, GrantedSkill, ServiceId, SkillId, SoftwarePackage,
};

use crate::environment_values::{MAX_ENTRIES, MAX_NAME, MAX_VALUE};
use crate::options::{CredentialMode, PermissionMode, ReasoningEffort, SessionMode};

/// How the agent behaves in a session started from this profile.
///
/// These were once chosen per session, which meant the same decisions were
/// made again on every start and a session could not be described by anything
/// reusable. They belong to the bundle: "which agent, thinking how hard,
/// asking permission how often, spending whose model access, carrying which
/// skills" is one coherent answer, and naming it is what makes it repeatable.
///
/// Skills sit here rather than beside software because a skill is something
/// the agent knows how to do, not something installed in the box around it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentConfiguration {
    /// Which runtime implementation backs the session.
    ///
    /// Unset by default, and deliberately: which runtimes exist, and which one
    /// an unqualified profile should use, is a decision about a deployment.
    /// Naming one here would put a particular runtime's name in the layer that
    /// exists precisely so nothing depends on which runtime is running. It is
    /// settled from configuration on the way in.
    pub kernel: KernelId,
    /// Which plane sessions from this profile are driven on.
    ///
    /// Part of the profile rather than the request for the same reason
    /// everything else here is: two sessions started from one profile must be
    /// the same session twice, and what plane a session runs on is the largest
    /// difference there could be between them.
    #[serde(default)]
    pub mode: SessionMode,
    /// Model the agent selects, when the profile pins one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// How hard the agent thinks before answering.
    #[serde(default)]
    pub reasoning_effort: ReasoningEffort,
    /// How often it stops to ask.
    #[serde(default)]
    pub permission: PermissionMode,
    /// Whose model access it spends.
    #[serde(default)]
    pub credential: CredentialMode,
    /// How much a session started from this profile reserves.
    ///
    /// Part of the profile for the reason everything else here is: two
    /// sessions started from one profile must be the same session twice, and
    /// a session that got half the memory is not.
    ///
    /// `None` means this deployment's own reservation, which is what every
    /// profile stored before this field meant and still means. It is not zero,
    /// and it is not a size this layer invented.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<crate::size::SessionSize>,
    /// Whether a session's tool bodies are recorded in its durable log.
    ///
    /// Off by default for the same reason it is on the session's own options:
    /// a tool's output can contain a value the session was handed, so recording
    /// it is a choice the profile makes rather than something it gets by
    /// omission.
    #[serde(default)]
    pub record_tool_bodies: bool,
    /// Skills loaded into the session.
    #[serde(default)]
    pub skills: Vec<SkillId>,
}

/// What a caller supplies when creating or replacing a profile.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvironmentProfileSpec {
    /// Name the owner recognises it by.
    pub name: String,
    /// How the agent behaves.
    #[serde(default)]
    pub agent: AgentConfiguration,
    /// Variables to set in the session.
    #[serde(default)]
    pub variables: Vec<EnvironmentVariable>,
    /// Software the session should carry.
    #[serde(default)]
    pub packages: Vec<SoftwarePackage>,
    /// Which of the owner's platform services this session may reach.
    ///
    /// A session acts on its owner's behalf, and until now it did so with the
    /// owner's whole identity — everything they can reach, whether or not the
    /// work called for it. Naming services here is what narrows that to the
    /// ones the work actually needs.
    ///
    /// What the deployment mandates is added whatever this says, because a
    /// session that could not reach its own model or skill registry would not
    /// be a narrower session but a broken one.
    #[serde(default)]
    pub services: Vec<ServiceId>,
    /// Standing instructions the owner wants every session to start with.
    ///
    /// Theirs alone. What this platform has to tell an agent — that it is
    /// already authenticated, which services it may reach, which commands
    /// will refuse it — is composed separately and cannot be edited from
    /// here, because a session that was talked out of knowing those things
    /// spends its first minutes rediscovering them badly.
    ///
    /// Empty is the ordinary case: a profile that adds nothing still gets
    /// everything the platform says.
    #[serde(default)]
    pub agent_notes: String,
}

impl EnvironmentProfileSpec {
    /// Reject anything that cannot be stored or handed to a runtime.
    ///
    /// This baseline-free form is useful to code that owns no deployment
    /// configuration. Profile services use `validate_with_baseline` so a
    /// normalized response remains echoable after mandatory entries are added.
    pub fn validate(&self) -> Result<(), CoreError> {
        self.validate_with_baseline(&Baseline::default())
    }

    /// Reject invalid content while treating one copy of each mandatory entry
    /// as deployment-owned rather than caller-controlled.
    ///
    /// GET includes today's baseline, so only one occurrence per distinct
    /// current mandatory identity is discounted. Repeated mandatory identifiers
    /// and identities retired from the current baseline consume the caller's
    /// bound. The profile service separately permits an exact normalized no-op
    /// before this validation; every changed write is checked here.
    pub fn validate_with_baseline(&self, baseline: &Baseline) -> Result<(), CoreError> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err(CoreError::InvalidRequest("name must not be empty".into()));
        }
        if name.chars().count() > MAX_NAME {
            return Err(CoreError::InvalidRequest(format!(
                "name must be at most {MAX_NAME} characters"
            )));
        }
        check_length("variables", self.variables.len())?;

        let mut seen: Vec<&str> = Vec::with_capacity(self.variables.len());
        for variable in &self.variables {
            if !EnvironmentVariable::is_valid_key(&variable.key) {
                return Err(CoreError::InvalidRequest(format!(
                    "{:?} is not a usable variable name",
                    variable.key
                )));
            }
            if variable.value.len() > MAX_VALUE {
                // The key names the offender; the value is never echoed.
                return Err(CoreError::InvalidRequest(format!(
                    "value of {} exceeds {MAX_VALUE} bytes",
                    variable.key
                )));
            }
            if seen.contains(&variable.key.as_str()) {
                return Err(CoreError::InvalidRequest(format!(
                    "{} is set twice",
                    variable.key
                )));
            }
            seen.push(&variable.key);
        }

        for package in &self.packages {
            if package.name.trim().is_empty() {
                return Err(CoreError::InvalidRequest(
                    "a package name must not be empty".into(),
                ));
            }
        }
        for service in &self.services {
            if service.0.trim().is_empty() {
                return Err(CoreError::InvalidRequest(
                    "a service identifier must not be empty".into(),
                ));
            }
        }
        for skill in &self.agent.skills {
            if skill.0.trim().is_empty() {
                return Err(CoreError::InvalidRequest(
                    "a skill identifier must not be empty".into(),
                ));
            }
        }

        check_baseline_aware_length(
            "packages",
            self.packages.iter().map(|package| package.name.as_str()),
            baseline
                .packages
                .iter()
                .map(|package| package.name.as_str()),
        )?;
        check_baseline_aware_length(
            "skills",
            self.agent.skills.iter().map(|skill| skill.0.as_str()),
            baseline.skills.iter().map(|skill| skill.0.as_str()),
        )?;
        check_baseline_aware_length(
            "services",
            self.services.iter().map(|service| service.0.as_str()),
            baseline.services.iter().map(|service| service.0.as_str()),
        )?;
        Ok(())
    }
}

fn check_baseline_aware_length<'entry, 'mandatory>(
    what: &str,
    entries: impl IntoIterator<Item = &'entry str>,
    mandatory: impl IntoIterator<Item = &'mandatory str>,
) -> Result<(), CoreError> {
    let mandatory: HashSet<&str> = mandatory.into_iter().collect();
    let mut discounted = HashSet::new();
    let caller_controlled = entries
        .into_iter()
        .filter(|entry| !(mandatory.contains(*entry) && discounted.insert(*entry)))
        .count();
    check_length(what, caller_controlled)
}

fn check_length(what: &str, len: usize) -> Result<(), CoreError> {
    if len > MAX_ENTRIES {
        return Err(CoreError::InvalidRequest(format!(
            "at most {MAX_ENTRIES} {what} are allowed, got {len}"
        )));
    }
    Ok(())
}

/// A stored environment profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentProfile {
    /// Stable identifier.
    pub id: EnvironmentProfileId,
    /// The subject that owns it. Every operation is scoped to this subject.
    pub owner: SubjectId,
    /// Name the owner recognises it by.
    pub name: String,
    /// How the agent behaves in sessions started with this profile.
    #[serde(default)]
    pub agent: AgentConfiguration,
    /// Variables set in every session started with this profile.
    pub variables: Vec<EnvironmentVariable>,
    /// Software the session carries.
    pub packages: Vec<SoftwarePackage>,
    /// Platform services a session started with this profile may reach.
    #[serde(default)]
    pub services: Vec<ServiceId>,
    /// Standing instructions the owner wants every session to start with.
    ///
    /// Theirs alone. What this platform has to tell an agent — that it is
    /// already authenticated, which services it may reach, which commands
    /// will refuse it — is composed separately and cannot be edited from
    /// here, because a session that was talked out of knowing those things
    /// spends its first minutes rediscovering them badly.
    ///
    /// Empty is the ordinary case: a profile that adds nothing still gets
    /// everything the platform says.
    #[serde(default)]
    pub agent_notes: String,
    /// Creation timestamp, RFC 3339 in UTC.
    pub created_at: String,
    /// Last modification timestamp, RFC 3339 in UTC.
    pub updated_at: String,
    /// Whether clients should preselect this profile for its owner.
    ///
    /// At most one of a subject's profiles holds it. Kept here rather than as
    /// a pointer beside the subject because it is a fact about the profile
    /// that is read whenever the profile is — a separate record would have to
    /// be fetched to answer a question the list already answers.
    ///
    /// This is presentation state only. Session creation never consults it: an
    /// unqualified create uses deployment defaults, and a qualified create uses
    /// the id in that request. `#[serde(default)]` keeps older stored profiles
    /// readable as not preselected.
    #[serde(default)]
    pub preferred: bool,
}

impl EnvironmentProfile {
    /// Whether `subject` may operate on this profile.
    ///
    /// Ownership is the whole model, and unlike a session there is no
    /// privileged cross-subject read: a profile's values are the one place a
    /// caller is invited to store a credential.
    pub fn is_owned_by(&self, subject: &SubjectId) -> bool {
        &self.owner == subject
    }

    /// The profile's contents, in the shape a caller would have supplied.
    ///
    /// The bridge back to a spec, so re-applying the baseline to something
    /// already stored goes through exactly the same code as applying it to a
    /// fresh request.
    pub fn to_spec(&self) -> EnvironmentProfileSpec {
        EnvironmentProfileSpec {
            name: self.name.clone(),
            agent: self.agent.clone(),
            variables: self.variables.clone(),
            packages: self.packages.clone(),
            services: self.services.clone(),
            agent_notes: self.agent_notes.clone(),
        }
    }

    /// Replace caller-controlled content without touching server-owned state.
    ///
    /// Store implementations use this in their in-memory doubles to mirror the
    /// atomic database operation. Identity, ownership, creation time, and
    /// client preselection remain exactly as they are at the instant of the
    /// replacement.
    pub fn apply_content(&mut self, content: &EnvironmentProfileSpec, updated_at: &str) {
        self.name = content.name.clone();
        self.agent = content.agent.clone();
        self.variables = content.variables.clone();
        self.packages = content.packages.clone();
        self.services = content.services.clone();
        self.agent_notes = content.agent_notes.clone();
        self.updated_at = updated_at.to_string();
    }
}

/// The entries every session carries whether or not anyone asked for them.
///
/// Injected rather than hard-coded: which software and which skills are part
/// of the platform is a deployment fact, while "these cannot be removed" is a
/// rule of the core. Keeping the two apart is what lets the list change
/// without the guarantee changing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Baseline {
    packages: Vec<SoftwarePackage>,
    skills: Vec<SkillId>,
    services: Vec<ServiceId>,
}

impl Baseline {
    /// Build a baseline from what a deployment mandates.
    pub fn new(
        packages: impl IntoIterator<Item = SoftwarePackage>,
        skills: impl IntoIterator<Item = SkillId>,
        services: impl IntoIterator<Item = ServiceId>,
    ) -> Self {
        Self {
            packages: unique_by_identity(packages, |package| package.name.clone()),
            skills: unique_by_identity(skills, |skill| skill.0.clone()),
            services: unique_by_identity(services, |service| service.0.clone()),
        }
    }

    /// Services every session may reach, whatever its profile says.
    ///
    /// The floor rather than a default: one credential serves the agent's own
    /// model calls and the platform CLI alike, so a profile that excluded the
    /// model endpoint would leave the session unable to think rather than
    /// merely narrower.
    pub fn services(&self) -> &[ServiceId] {
        &self.services
    }

    /// Software every session carries.
    pub fn packages(&self) -> &[SoftwarePackage] {
        &self.packages
    }

    /// Skills every session loads.
    pub fn skills(&self) -> &[SkillId] {
        &self.skills
    }

    /// Whether this package is one a caller may not remove.
    pub fn requires_package(&self, name: &str) -> bool {
        self.packages.iter().any(|p| p.name == name)
    }

    /// Whether this skill is one a caller may not remove.
    pub fn requires_skill(&self, id: &SkillId) -> bool {
        self.skills.contains(id)
    }

    /// Put the mandatory entries back into a caller's request.
    ///
    /// Mandatory entries lead, so a reader sees what the platform contributed
    /// before what the owner chose. A caller who did list one keeps their own
    /// entry — pinning a version of a mandatory package is a legitimate choice,
    /// and duplicating it would install it twice.
    pub fn apply(&self, spec: &mut EnvironmentProfileSpec) {
        let chosen_packages = std::mem::take(&mut spec.packages);
        for mandatory in &self.packages {
            if !chosen_packages.iter().any(|p| p.name == mandatory.name) {
                spec.packages.push(mandatory.clone());
            }
        }
        spec.packages.extend(chosen_packages);

        let chosen_skills = std::mem::take(&mut spec.agent.skills);
        for mandatory in &self.skills {
            if !chosen_skills.contains(mandatory) {
                spec.agent.skills.push(mandatory.clone());
            }
        }
        spec.agent.skills.extend(chosen_skills);

        // Same rule for services, and for a sharper reason: one credential
        // serves every call a session makes, so a mandatory service left out
        // is not a narrower session but one that cannot reach its own model.
        let chosen_services = std::mem::take(&mut spec.services);
        for mandatory in &self.services {
            if !chosen_services.contains(mandatory) {
                spec.services.push(mandatory.clone());
            }
        }
        spec.services.extend(chosen_services);
    }
}

fn unique_by_identity<T>(
    entries: impl IntoIterator<Item = T>,
    identity: impl Fn(&T) -> String,
) -> Vec<T> {
    let mut seen = HashSet::new();
    entries
        .into_iter()
        .filter(|entry| seen.insert(identity(entry)))
        .collect()
}

/// Everything a session's runtime is given, once a selection has been resolved.
///
/// Variables are a map because a session cannot hold the same name twice, and
/// ordered so that what a runtime receives does not depend on iteration order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedEnvironment {
    /// Variables to set, by name.
    pub variables: BTreeMap<String, String>,
    /// Software requirements, as the installer names them.
    pub packages: Vec<String>,
    /// Skill identifiers to load.
    pub skills: Vec<String>,
    /// Standing instructions the owner wrote, carried verbatim.
    ///
    /// Not read here. What the platform tells an agent about itself is
    /// composed where the hosting facts are known, and this is appended to it
    /// rather than merged with it — the owner adds to what a session is told
    /// and cannot edit it.
    pub agent_notes: String,
}

impl ResolvedEnvironment {
    /// Render a set of selections that already has the baseline merged in.
    ///
    /// Takes a spec rather than a stored profile so that resolving something
    /// stored and resolving something just supplied go through one code path.
    pub fn from_spec(spec: &EnvironmentProfileSpec) -> Self {
        Self {
            variables: spec
                .variables
                .iter()
                .map(|v| (v.key.clone(), v.value.clone()))
                .collect(),
            packages: spec.packages.iter().map(|p| p.requirement()).collect(),
            skills: spec.agent.skills.iter().map(|s| s.0.clone()).collect(),
            agent_notes: spec.agent_notes.clone(),
        }
    }

    /// What a session gets when no profile was selected.
    ///
    /// Not empty: the baseline is what the platform mandates, and opting out of
    /// choosing a profile is not opting out of the platform.
    pub fn baseline_only(baseline: &Baseline) -> Self {
        Self {
            // No profile means no instructions of the owner's own. What the
            // platform says is added later either way.
            agent_notes: String::new(),
            variables: BTreeMap::new(),
            packages: baseline
                .packages()
                .iter()
                .map(|p| p.requirement())
                .collect(),
            skills: baseline.skills().iter().map(|s| s.0.clone()).collect(),
        }
    }
}

impl From<ResolvedEnvironment> for layer3_adapter::RuntimeEnvironment {
    /// Hand the resolved bundle to the layer that knows how to apply it.
    ///
    /// The core states intent; rendering it into a sandbox's environment is the
    /// adapter's job, and this conversion is the seam between the two.
    fn from(value: ResolvedEnvironment) -> Self {
        Self {
            variables: value.variables,
            packages: value.packages,
            skills: value.skills,
            agent_notes: value.agent_notes,
        }
    }
}

#[cfg(test)]
#[path = "environment_tests.rs"]
mod tests;
