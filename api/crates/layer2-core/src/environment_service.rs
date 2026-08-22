//! Owning environment profiles: create, read, replace, remove, resolve.
//!
//! Separate from the session service because a profile is a separate resource
//! with its own lifetime — it is authored once and reused, and outlives every
//! session that referenced it.
//!
//! Two rules live here and nowhere else, so no interface can forget either:
//!
//! * **Scoped to the owner.** Every operation reads the record first and
//!   refuses unless the caller owns it. There is no privileged cross-subject
//!   read, unlike sessions: a profile's values are where a caller is invited to
//!   put a credential.
//! * **The baseline is not optional.** It is merged on write so a stored
//!   profile reads back honestly, and again on resolve so a profile written
//!   before the platform mandated something still gets it.

use std::sync::Arc;

use crate::authz::{Caller, Capability};
use crate::clock::now_rfc3339;
use crate::environment::{
    AgentConfiguration, Baseline, CatalogSkill, EnvironmentProfile, EnvironmentProfileSpec,
    ResolvedEnvironment, ServiceAvailability, SkillCatalog,
};
use crate::error::CoreError;
use crate::ids::{EnvironmentProfileId, SubjectId};
use crate::platform_service::PlatformSettingsService;
use crate::store::{EnvironmentProfileStore, StoreError};

#[path = "environment_warnings.rs"]
mod warnings;

/// Reusable environment bundles, and what a session is started with.
pub struct EnvironmentProfileService {
    store: Arc<dyn EnvironmentProfileStore>,
    /// What every session carries whether or not anyone asked for it.
    baseline: Baseline,
    /// Where the services a caller may allow are listed, when there is one.
    services: Option<Arc<dyn crate::environment::ServiceCatalog>>,
    /// How the agent behaves in a session that names no profile.
    ///
    /// The deployment's answer rather than a hardcoded one: which model is
    /// worth spending and how freely an agent may act are decisions about this
    /// installation, and a session started without a profile still has to be
    /// started somehow.
    default_agent: AgentConfiguration,
    /// Where selectable skills are listed, when a deployment has one.
    ///
    /// Optional: without it profiles still store and resolve skill identifiers,
    /// there is simply no list to choose from. That is a degraded picker, not a
    /// broken session.
    catalog: Option<Arc<dyn SkillCatalog>>,
    /// Deployment policy consulted only to warn about a stated profile size.
    settings: Arc<PlatformSettingsService>,
}

impl EnvironmentProfileService {
    /// Build the service over its ports.
    pub fn new(
        store: Arc<dyn EnvironmentProfileStore>,
        baseline: Baseline,
        catalog: Option<Arc<dyn SkillCatalog>>,
        services: Option<Arc<dyn crate::environment::ServiceCatalog>>,
        default_agent: AgentConfiguration,
        settings: Arc<PlatformSettingsService>,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            baseline,
            catalog,
            services,
            default_agent,
            settings,
        })
    }

    /// Services this caller may allow a session to reach.
    ///
    /// An empty list where no catalogue is configured, rather than a failure:
    /// a deployment without a picker still stores and honours selections, it
    /// simply has nothing to offer.
    pub async fn available_services(
        &self,
        caller: &Caller,
        credential: Option<&str>,
    ) -> Result<Vec<crate::environment::CatalogService>, CoreError> {
        Ok(self
            .service_availability(caller, credential)
            .await?
            .listed())
    }

    /// What the catalogue says, keeping its three answers apart.
    ///
    /// "You have granted nothing", "we could not ask right now" and "this
    /// deployment has no catalogue" are three different facts, and a caller who
    /// cannot tell them apart does the wrong thing for two of them: they retry
    /// the one that will never succeed, and give up on the one that would have
    /// worked a moment later.
    ///
    /// Collapsing them is what the old signature did — an empty list meant
    /// both "no catalogue" and "nothing granted", and any failure to reach it
    /// arrived as `RuntimeUnavailable`, which sends a caller to look at the
    /// sandbox provider.
    pub async fn service_availability(
        &self,
        caller: &Caller,
        credential: Option<&str>,
    ) -> Result<ServiceAvailability, CoreError> {
        caller
            .authorization
            .require(Capability::ManageOwnEnvironmentProfiles)?;
        let Some(catalog) = self.services.as_ref() else {
            return Ok(ServiceAvailability::NoCatalogue);
        };
        catalog
            .available(&caller.subject, credential)
            .await
            .map(ServiceAvailability::Granted)
            .map_err(|e| CoreError::ServiceCatalogUnavailable(e.to_string()))
    }

    /// Refuse a selection naming a service this owner has not granted.
    ///
    /// **At provision, not at write.** A grant can be revoked between the two,
    /// so the write-time answer is friendlier and the provision-time answer is
    /// the true one — a profile that was valid when it was saved may name
    /// something its owner has since taken back, and the session that would
    /// have been started against it must be refused rather than started and
    /// left failing every call from inside the sandbox.
    ///
    /// **A catalogue that cannot be asked refuses nothing.** What a session may
    /// reach is recorded on the profile; the catalogue exists to help choose,
    /// not to authorise. Blocking a session because NyxID was briefly
    /// unreachable would take the service down with it, and would do so for
    /// profiles whose services were settled days ago.
    ///
    /// A deployment with no catalogue refuses nothing for the same reason: it
    /// has withheld nothing, so there is nothing it can be said to have denied.
    ///
    /// The baseline is never checked. Those services are what a session *is*
    /// on this platform rather than something its owner opted into, and a
    /// deployment that mandates one has already decided.
    pub async fn refuse_ungranted(
        &self,
        caller: &Caller,
        credential: Option<&str>,
        selected: &[crate::environment::ServiceId],
    ) -> Result<(), CoreError> {
        let wanted: Vec<_> = selected
            .iter()
            .filter(|id| !self.baseline.services().contains(id))
            .collect();
        if wanted.is_empty() {
            return Ok(());
        }

        let availability = match self.service_availability(caller, credential).await {
            Ok(availability) => availability,
            Err(CoreError::ServiceCatalogUnavailable(reason)) => {
                tracing::warn!(
                    subject = %caller.subject,
                    reason = %reason,
                    "the service catalogue could not be asked; the profile's stored selection stands"
                );
                return Ok(());
            }
            Err(other) => return Err(other),
        };
        let Some(granted) = availability.grants() else {
            return Ok(());
        };

        for id in wanted {
            if !granted.iter().any(|service| &service.id == id) {
                tracing::info!(
                    subject = %caller.subject,
                    service = %id.0,
                    "refused a session naming a service its owner has not granted"
                );
                return Err(CoreError::ServiceNotGranted(id.0.clone()));
            }
        }
        Ok(())
    }

    /// What this deployment mandates in every session.
    pub fn baseline(&self) -> &Baseline {
        &self.baseline
    }

    /// How the agent behaves when a session names no profile.
    pub fn default_agent(&self) -> &AgentConfiguration {
        &self.default_agent
    }

    /// Fill in the kernel when a caller did not name one.
    ///
    /// The core holds no opinion about which kernels exist, so a profile that
    /// named none arrives unset and is settled here from what this deployment
    /// configured. Doing it on the way in rather than on the way out means a
    /// stored profile says what it will actually run, and reads the same
    /// tomorrow as it does today.
    fn settle_kernel(&self, spec: &mut EnvironmentProfileSpec) {
        if spec.agent.kernel.is_unset() {
            spec.agent.kernel = self.default_agent.kernel.clone();
        }
    }

    /// Which services a session started with this selection may reach.
    ///
    /// Always includes what the deployment mandates, whichever profile is
    /// named and whether one is named at all. A profile that has since been
    /// deleted leaves the floor rather than nothing: a session already running
    /// must not lose its model endpoint because a bundle was tidied up.
    pub async fn services_for(
        &self,
        owner: &SubjectId,
        selection: Option<&EnvironmentProfileId>,
    ) -> Vec<crate::environment::ServiceId> {
        let mut chosen: Vec<crate::environment::ServiceId> = match selection {
            Some(id) => match self.store.get(id).await {
                Ok(Some(profile)) if profile.is_owned_by(owner) => profile.services.clone(),
                _ => Vec::new(),
            },
            None => Vec::new(),
        };
        for mandatory in self.baseline.services() {
            if !chosen.contains(mandatory) {
                chosen.push(mandatory.clone());
            }
        }
        chosen
    }

    /// How the agent should behave for a session started with this selection.
    ///
    /// The profile is the whole answer. A caller does not get to say which
    /// agent, which model, or how freely it may act — those are the profile's
    /// to state, which is what makes a session reproducible from the name of
    /// the bundle it was started with.
    ///
    /// Naming nothing is a choice too, and it resolves to what the deployment
    /// considers ordinary rather than to an empty configuration.
    pub async fn agent_for(
        &self,
        caller: &Caller,
        selection: Option<&EnvironmentProfileId>,
    ) -> Result<AgentConfiguration, CoreError> {
        let Some(id) = selection else {
            return Ok(self.default_agent.clone());
        };
        let mut agent = self.get(caller, id).await?.agent;
        // A profile stored before the kernel was settled on the way in. It
        // runs what this deployment runs rather than nothing at all.
        if agent.kernel.is_unset() {
            agent.kernel = self.default_agent.kernel.clone();
        }
        Ok(agent)
    }

    /// Store a new profile for the caller.
    pub async fn create(
        &self,
        caller: &Caller,
        mut spec: EnvironmentProfileSpec,
    ) -> Result<EnvironmentProfile, CoreError> {
        caller
            .authorization
            .require(Capability::ManageOwnEnvironmentProfiles)?;
        // Validated before the baseline is merged, so a caller is never told
        // their request is too large because of entries the platform added.
        spec.validate()?;
        self.baseline.apply(&mut spec);
        self.settle_kernel(&mut spec);

        let now = now_rfc3339();
        let profile = EnvironmentProfile {
            id: EnvironmentProfileId::generate(),
            owner: caller.subject.clone(),
            name: spec.name.trim().to_string(),
            variables: spec.variables,
            packages: spec.packages,
            agent: spec.agent,
            services: spec.services,
            agent_notes: spec.agent_notes,
            created_at: now.clone(),
            updated_at: now,
            // A new profile is not its owner's default. Making one so is a
            // decision they take, and taking it for them would move where
            // every later session starts as a side effect of authoring.
            preferred: false,
        };
        self.store.upsert(&profile).await.map_err(store_failed)?;
        // Noted after the write, never instead of it: a selection the
        // catalogue does not currently grant is a warning here and a refusal
        // at provision, because a grant can change between the two.
        self.warn_ungranted(caller, None, &profile.services).await;
        self.warn_out_of_bounds(caller, profile.agent.size).await;
        tracing::info!(
            profile_id = %profile.id,
            owner = %profile.owner,
            variables = profile.variables.len(),
            packages = profile.packages.len(),
            skills = profile.agent.skills.len(),
            "environment profile created"
        );
        Ok(profile)
    }

    /// Read one of the caller's profiles.
    pub async fn get(
        &self,
        caller: &Caller,
        id: &EnvironmentProfileId,
    ) -> Result<EnvironmentProfile, CoreError> {
        caller
            .authorization
            .require(Capability::ManageOwnEnvironmentProfiles)?;
        self.owned(caller, id).await
    }

    /// Every profile the caller owns, newest first.
    pub async fn list(&self, caller: &Caller) -> Result<Vec<EnvironmentProfile>, CoreError> {
        caller
            .authorization
            .require(Capability::ManageOwnEnvironmentProfiles)?;
        let mut profiles = self
            .store
            .list_for(&caller.subject)
            .await
            .map_err(store_failed)?;
        settle_preferred(&mut profiles);
        Ok(profiles)
    }

    /// Make one of the caller's profiles the one their sessions start from.
    ///
    /// At most one may hold it, so the others are cleared in the same call.
    /// There is no transaction across the store port, so this is written to be
    /// safe without one: a write that half-succeeds leaves either two claims or
    /// none, and both are resolved on read rather than left to be discovered —
    /// see `settle_preferred`.
    ///
    /// Passing `None` clears the preference, which is a real choice: it returns
    /// the caller to the deployment's own defaults rather than being the
    /// absence of one.
    pub async fn prefer(
        &self,
        caller: &Caller,
        id: Option<&EnvironmentProfileId>,
    ) -> Result<Vec<EnvironmentProfile>, CoreError> {
        caller
            .authorization
            .require(Capability::ManageOwnEnvironmentProfiles)?;
        // Read through `owned` so a profile the caller does not own is refused
        // by the same rule every other operation is refused by.
        if let Some(wanted) = id {
            self.owned(caller, wanted).await?;
        }

        let mut profiles = self
            .store
            .list_for(&caller.subject)
            .await
            .map_err(store_failed)?;
        for profile in &mut profiles {
            let wanted = id.is_some_and(|chosen| &profile.id == chosen);
            if profile.preferred == wanted {
                continue;
            }
            profile.preferred = wanted;
            self.store.upsert(profile).await.map_err(store_failed)?;
        }
        tracing::info!(
            owner = %caller.subject,
            profile_id = id.map(|chosen| chosen.0.as_str()).unwrap_or("<none>"),
            "the profile this owner's sessions start from"
        );
        settle_preferred(&mut profiles);
        Ok(profiles)
    }

    /// Replace a profile's contents, keeping its identity and creation time.
    pub async fn update(
        &self,
        caller: &Caller,
        id: &EnvironmentProfileId,
        mut spec: EnvironmentProfileSpec,
    ) -> Result<EnvironmentProfile, CoreError> {
        caller
            .authorization
            .require(Capability::ManageOwnEnvironmentProfiles)?;
        let existing = self.owned(caller, id).await?;
        spec.validate()?;
        // Applied again on every write: this is what makes removing a mandatory
        // entry impossible rather than merely discouraged.
        self.baseline.apply(&mut spec);

        let profile = EnvironmentProfile {
            id: existing.id,
            owner: existing.owner,
            name: spec.name.trim().to_string(),
            variables: spec.variables,
            packages: spec.packages,
            agent: spec.agent,
            services: spec.services,
            agent_notes: spec.agent_notes,
            created_at: existing.created_at,
            updated_at: now_rfc3339(),
            // Carried across an edit. Editing a profile is not choosing where
            // sessions start from, and silently clearing it would move that
            // the next time somebody fixed a typo in its name.
            preferred: existing.preferred,
        };
        self.store.upsert(&profile).await.map_err(store_failed)?;
        self.warn_ungranted(caller, None, &profile.services).await;
        self.warn_out_of_bounds(caller, profile.agent.size).await;
        tracing::info!(profile_id = %profile.id, owner = %profile.owner, "environment profile replaced");
        Ok(profile)
    }

    /// Remove one of the caller's profiles.
    pub async fn delete(
        &self,
        caller: &Caller,
        id: &EnvironmentProfileId,
    ) -> Result<(), CoreError> {
        caller
            .authorization
            .require(Capability::ManageOwnEnvironmentProfiles)?;
        // Read first: deleting by id alone would let one subject remove
        // another's profile by guessing an identifier.
        self.owned(caller, id).await?;
        self.store.delete(id).await.map_err(store_failed)?;
        tracing::info!(profile_id = %id, owner = %caller.subject, "environment profile removed");
        Ok(())
    }

    /// What a session started by this caller should be given.
    ///
    /// A caller who selected nothing still gets the baseline, and a caller who
    /// selected a profile written before the baseline changed gets the current
    /// one — resolving is the last point at which the platform's mandate can be
    /// applied, so it is applied here regardless of what was stored.
    pub async fn resolve(
        &self,
        caller: &Caller,
        selection: Option<&EnvironmentProfileId>,
    ) -> Result<ResolvedEnvironment, CoreError> {
        let Some(id) = selection else {
            return Ok(ResolvedEnvironment::baseline_only(&self.baseline));
        };
        // Selecting a profile is using it, which is a read of the caller's own
        // resource — the same capability that governs every other read of it.
        caller
            .authorization
            .require(Capability::ManageOwnEnvironmentProfiles)?;
        let profile = self.owned(caller, id).await?;
        let mut spec = profile.to_spec();
        self.baseline.apply(&mut spec);
        let resolved = ResolvedEnvironment::from_spec(&spec);
        tracing::debug!(
            profile_id = %id,
            variables = resolved.variables.len(),
            packages = resolved.packages.len(),
            skills = resolved.skills.len(),
            "environment profile resolved"
        );
        Ok(resolved)
    }

    /// Resolve on an owner's behalf, with no request behind it.
    ///
    /// The controller re-provisions and rotates sessions long after the request
    /// that created them, so there is no caller to check — the check already
    /// happened when the session recorded which profile it uses. Ownership is
    /// still enforced: a profile that has since changed hands stops applying,
    /// which is the safe direction.
    pub async fn resolve_for_owner(
        &self,
        owner: &SubjectId,
        selection: Option<&EnvironmentProfileId>,
    ) -> Result<ResolvedEnvironment, CoreError> {
        let Some(id) = selection else {
            return Ok(ResolvedEnvironment::baseline_only(&self.baseline));
        };
        let profile = self
            .store
            .get(id)
            .await
            .map_err(store_failed)?
            .ok_or(CoreError::EnvironmentProfileNotFound)?;
        if !profile.is_owned_by(owner) {
            tracing::warn!(
                profile_id = %id,
                %owner,
                "environment profile no longer belongs to this session's owner"
            );
            return Err(CoreError::EnvironmentProfileNotFound);
        }
        let mut spec = profile.to_spec();
        self.baseline.apply(&mut spec);
        Ok(ResolvedEnvironment::from_spec(&spec))
    }

    /// Skills this caller may choose from.
    ///
    /// The credential is the caller's own, when one has been delegated; it is
    /// what lets the catalogue answer with what this subject can see rather
    /// than only what is public.
    pub async fn available_skills(
        &self,
        caller: &Caller,
        credential: Option<&str>,
    ) -> Result<Vec<CatalogSkill>, CoreError> {
        caller
            .authorization
            .require(Capability::ManageOwnEnvironmentProfiles)?;
        let Some(catalog) = self.catalog.as_ref() else {
            // No catalogue configured is not a failure: profiles still work,
            // the caller simply has nothing to browse.
            tracing::debug!("no skill catalogue is configured");
            return Ok(Vec::new());
        };
        catalog
            .available(&caller.subject, credential)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "cannot list selectable skills");
                CoreError::RuntimeUnavailable(format!("skill catalogue: {e}"))
            })
    }

    /// Read a profile and refuse unless this caller owns it.
    ///
    /// "Not yours" and "does not exist" collapse into one condition on purpose:
    /// distinguishing them would confirm an identifier to someone who may not
    /// see it.
    async fn owned(
        &self,
        caller: &Caller,
        id: &EnvironmentProfileId,
    ) -> Result<EnvironmentProfile, CoreError> {
        let profile = self
            .store
            .get(id)
            .await
            .map_err(store_failed)?
            .ok_or(CoreError::EnvironmentProfileNotFound)?;
        if !profile.is_owned_by(&caller.subject) {
            tracing::warn!(
                profile_id = %id,
                subject = %caller.subject,
                "refused access to another subject's environment profile"
            );
            return Err(CoreError::EnvironmentProfileNotFound);
        }
        Ok(profile)
    }
}

/// Render a storage failure as a core error.
///
/// Unreachable storage is reported as unavailability rather than as "not
/// found", so a caller is never told their profile is gone when it is merely
/// out of reach.
fn store_failed(e: StoreError) -> CoreError {
    CoreError::RuntimeUnavailable(format!("environment profile store: {e}"))
}

#[cfg(test)]
#[path = "environment_resolution_tests.rs"]
mod environment_resolution_tests;

#[cfg(test)]
#[path = "environment_service_tests.rs"]
mod tests;

/// Leave at most one profile claiming to be the owner's default.
///
/// The store has no transaction, so a half-finished `prefer` can leave two
/// claims. Rather than trusting the write, the read settles it: the most
/// recently updated wins, and the rest are reported as what they are. None
/// claiming it is not a fault — it means the owner has not chosen one, and
/// their sessions start from the deployment's own defaults.
fn settle_preferred(profiles: &mut [EnvironmentProfile]) {
    let winner = profiles
        .iter()
        .filter(|profile| profile.preferred)
        .max_by(|a, b| a.updated_at.cmp(&b.updated_at))
        .map(|profile| profile.id.clone());
    let Some(winner) = winner else {
        return;
    };
    for profile in profiles.iter_mut() {
        profile.preferred = profile.id == winner;
    }
}
