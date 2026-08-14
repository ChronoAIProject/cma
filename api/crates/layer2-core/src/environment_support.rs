//! Test doubles for environment profiles.
//!
//! Split from `test_support` so both stay within the project's file-size
//! limit. They belong to the same test-only surface and are re-exported from
//! there, so a test still has one place to import from.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::environment::{
    Baseline, CatalogError, CatalogSkill, EnvironmentProfile, EnvironmentProfileSpec, SkillCatalog,
    SkillId, SoftwarePackage,
};
use crate::environment_service::EnvironmentProfileService;
use crate::ids::{EnvironmentProfileId, SubjectId};
use crate::store::{EnvironmentProfileStore, StoreError};

/// Environment profiles held in memory, shared between service instances.
///
/// Shareable for the same reason the session store is: two services over one
/// store is how "a profile written by an older deployment" is expressible.
#[derive(Default)]
pub(crate) struct FakeProfileStore {
    pub(crate) profiles: Mutex<HashMap<String, EnvironmentProfile>>,
    /// When set, every operation fails, so callers can be checked against a
    /// store that is down rather than merely empty.
    pub(crate) broken: Mutex<bool>,
}

impl FakeProfileStore {
    pub(crate) async fn fail_everything(&self) {
        *self.broken.lock().await = true;
    }

    async fn guard(&self) -> Result<(), StoreError> {
        if *self.broken.lock().await {
            return Err(StoreError::Unavailable("test store is down".into()));
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl EnvironmentProfileStore for FakeProfileStore {
    async fn upsert(&self, profile: &EnvironmentProfile) -> Result<(), StoreError> {
        self.guard().await?;
        self.profiles
            .lock()
            .await
            .insert(profile.id.0.clone(), profile.clone());
        Ok(())
    }

    async fn get(
        &self,
        id: &EnvironmentProfileId,
    ) -> Result<Option<EnvironmentProfile>, StoreError> {
        self.guard().await?;
        Ok(self.profiles.lock().await.get(&id.0).cloned())
    }

    async fn list_for(&self, owner: &SubjectId) -> Result<Vec<EnvironmentProfile>, StoreError> {
        self.guard().await?;
        Ok(self
            .profiles
            .lock()
            .await
            .values()
            .filter(|p| &p.owner == owner)
            .cloned()
            .collect())
    }

    async fn delete(&self, id: &EnvironmentProfileId) -> Result<(), StoreError> {
        self.guard().await?;
        self.profiles.lock().await.remove(&id.0);
        Ok(())
    }
}

/// A catalogue that answers from a fixed list and records how it was called.
pub(crate) struct FakeSkillCatalog {
    skills: Vec<CatalogSkill>,
    failure: Option<CatalogError>,
    /// The credential the last caller was carrying, so it can be asserted that
    /// the caller's own identity reaches the catalogue rather than ours.
    pub(crate) last_credential: Mutex<Option<String>>,
}

impl FakeSkillCatalog {
    pub(crate) fn with_skills<'a>(ids: impl IntoIterator<Item = &'a str>) -> Self {
        Self {
            skills: ids
                .into_iter()
                .map(|id| CatalogSkill {
                    id: SkillId::new(id),
                    name: id.to_string(),
                    description: String::new(),
                    access: None,
                })
                .collect(),
            failure: None,
            last_credential: Mutex::new(None),
        }
    }

    pub(crate) fn broken(failure: CatalogError) -> Self {
        Self {
            skills: Vec::new(),
            failure: Some(failure),
            last_credential: Mutex::new(None),
        }
    }
}

#[async_trait::async_trait]
impl SkillCatalog for FakeSkillCatalog {
    async fn available(
        &self,
        _subject: &SubjectId,
        credential: Option<&str>,
    ) -> Result<Vec<CatalogSkill>, CatalogError> {
        *self.last_credential.lock().await = credential.map(str::to_string);
        match &self.failure {
            Some(CatalogError::Unavailable(m)) => Err(CatalogError::Unavailable(m.clone())),
            Some(CatalogError::Malformed(m)) => Err(CatalogError::Malformed(m.clone())),
            None => Ok(self.skills.clone()),
        }
    }
}

/// The baseline the tests treat as what this deployment mandates.
pub(crate) fn baseline() -> Baseline {
    Baseline::new(
        [SoftwarePackage::new("platform-cli")],
        [SkillId::new("mandatory-one"), SkillId::new("mandatory-two")],
    )
}

/// A profile service over the given store, with the standard baseline.
pub(crate) fn environments(store: Arc<FakeProfileStore>) -> Arc<EnvironmentProfileService> {
    EnvironmentProfileService::new(store, baseline(), None, fake_agent())
}

/// A profile service wired to a catalogue.
pub(crate) fn catalog_service(catalog: Arc<FakeSkillCatalog>) -> Arc<EnvironmentProfileService> {
    EnvironmentProfileService::new(
        Arc::new(FakeProfileStore::default()),
        baseline(),
        Some(catalog),
        crate::environment::AgentConfiguration {
            // The kernel the fake adapter is registered under. The default is
            // the one this deployment ships, and a fixture that left it there
            // would be asking for a runtime the test never registered.
            kernel: crate::ids::KernelId::new("fake"),
            ..Default::default()
        },
    )
}

/// The agent configuration these fixtures can actually start.
///
/// The real default names the kernel this deployment ships, which no test
/// registers an adapter for. Naming the fake one here keeps every fixture
/// startable without each test having to know why.
pub(crate) fn fake_agent() -> crate::environment::AgentConfiguration {
    crate::environment::AgentConfiguration {
        kernel: crate::ids::KernelId::new("fake"),
        ..Default::default()
    }
}

/// A named profile with nothing else in it.
pub(crate) fn profile_spec(name: &str) -> EnvironmentProfileSpec {
    EnvironmentProfileSpec {
        name: name.to_string(),
        agent: fake_agent(),
        ..EnvironmentProfileSpec::default()
    }
}
