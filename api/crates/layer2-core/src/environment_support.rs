//! Test doubles for environment profiles.
//!
//! Split from `test_support` so both stay within the project's file-size
//! limit. They belong to the same test-only surface and are re-exported from
//! there, so a test still has one place to import from.

use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use tokio::sync::Mutex;

use crate::environment::{
    Baseline, CatalogError, CatalogSkill, EnvironmentProfile, EnvironmentProfileSpec, SkillCatalog,
    SkillId, SoftwarePackage,
};
use crate::environment_service::EnvironmentProfileService;
use crate::ids::{EnvironmentProfileId, SubjectId};
use crate::store::{EnvironmentProfileStore, Page, PageCursor, PageRequest, StoreError};

/// The shipped policy, for tests whose subject is not platform configuration.
pub(crate) fn unconfigured_settings() -> Arc<crate::PlatformSettingsService> {
    Arc::new(crate::PlatformSettingsService::unconfigured())
}

/// Environment profiles held in memory, shared between service instances.
///
/// Shareable for the same reason the session store is: two services over one
/// store is how "a profile written by an older deployment" is expressible.
#[derive(Default)]
pub(crate) struct FakeProfileStore {
    pub(crate) profiles: Mutex<HashMap<String, EnvironmentProfile>>,
    pub(crate) list_calls: AtomicUsize,
    pub(crate) preference_writes: AtomicUsize,
    pub(crate) content_replacements: AtomicUsize,
    replacement_interference: Mutex<Option<ReplacementInterference>>,
    /// When set, every operation fails, so callers can be checked against a
    /// store that is down rather than merely empty.
    pub(crate) broken: Mutex<bool>,
}

#[derive(Clone, Copy)]
pub(crate) enum ReplacementInterference {
    Preferred(bool),
    Delete,
}

impl FakeProfileStore {
    pub(crate) async fn fail_everything(&self) {
        *self.broken.lock().await = true;
    }

    pub(crate) async fn interfere_with_next_replacement(
        &self,
        interference: ReplacementInterference,
    ) {
        *self.replacement_interference.lock().await = Some(interference);
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

    async fn replace_content(
        &self,
        id: &EnvironmentProfileId,
        content: &EnvironmentProfileSpec,
        updated_at: &str,
    ) -> Result<Option<EnvironmentProfile>, StoreError> {
        self.guard().await?;
        let interference = self.replacement_interference.lock().await.take();
        let mut profiles = self.profiles.lock().await;
        match interference {
            Some(ReplacementInterference::Preferred(preferred)) => {
                if let Some(profile) = profiles.get_mut(&id.0) {
                    profile.preferred = preferred;
                }
            }
            Some(ReplacementInterference::Delete) => {
                profiles.remove(&id.0);
            }
            None => {}
        }
        let Some(profile) = profiles.get_mut(&id.0) else {
            return Ok(None);
        };
        profile.apply_content(content, updated_at);
        let profile = profile.clone();
        self.content_replacements.fetch_add(1, Ordering::SeqCst);
        Ok(Some(profile))
    }

    async fn get(
        &self,
        id: &EnvironmentProfileId,
    ) -> Result<Option<EnvironmentProfile>, StoreError> {
        self.guard().await?;
        Ok(self.profiles.lock().await.get(&id.0).cloned())
    }

    async fn list_for(
        &self,
        owner: &SubjectId,
        page: &PageRequest,
    ) -> Result<Page<EnvironmentProfile>, StoreError> {
        self.guard().await?;
        self.list_calls.fetch_add(1, Ordering::SeqCst);
        let mut profiles: Vec<_> = self
            .profiles
            .lock()
            .await
            .values()
            .filter(|p| &p.owner == owner)
            .filter(|profile| {
                page.cursor().is_none_or(|cursor| {
                    profile.created_at < cursor.created_at
                        || (profile.created_at == cursor.created_at && profile.id.0 < cursor.id)
                })
            })
            .cloned()
            .collect();
        profiles.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.id.0.cmp(&a.id.0))
        });
        let has_more = profiles.len() > page.limit() as usize;
        profiles.truncate(page.limit() as usize);
        let next = has_more.then(|| {
            let last = profiles
                .last()
                .expect("a non-empty bounded page has a last row");
            PageCursor::new(&last.created_at, &last.id.0)
        });
        Ok(Page {
            items: profiles,
            next,
        })
    }

    async fn set_preferred(
        &self,
        owner: &SubjectId,
        chosen: Option<&EnvironmentProfileId>,
    ) -> Result<(), StoreError> {
        self.guard().await?;
        let mut profiles = self.profiles.lock().await;
        if let Some(id) = chosen {
            let selected = profiles
                .get(&id.0)
                .filter(|profile| &profile.owner == owner);
            if selected.is_none() {
                return Err(StoreError::Malformed(
                    "selected environment profile is not owned by the subject".into(),
                ));
            }
        }
        for profile in profiles
            .values_mut()
            .filter(|profile| &profile.owner == owner)
        {
            profile.preferred = chosen.is_some_and(|id| profile.id == *id);
        }
        self.preference_writes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn preferred_for(
        &self,
        owner: &SubjectId,
    ) -> Result<Option<EnvironmentProfile>, StoreError> {
        self.guard().await?;
        Ok(self
            .profiles
            .lock()
            .await
            .values()
            .filter(|profile| &profile.owner == owner && profile.preferred)
            .max_by(|a, b| {
                a.updated_at
                    .cmp(&b.updated_at)
                    .then_with(|| a.id.0.cmp(&b.id.0))
            })
            .cloned())
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

/// A service catalogue that answers, or refuses, exactly as NyxID would.
///
/// The three answers it can give are the three this milestone exists to keep
/// apart: a list of grants, a refusal to answer, and — by not being wired up
/// at all — a deployment with no catalogue.
pub(crate) struct FakeServiceCatalog {
    granted: Vec<crate::environment::CatalogService>,
    failure: Option<CatalogError>,
}

impl FakeServiceCatalog {
    pub(crate) fn granting(ids: impl IntoIterator<Item = &'static str>) -> Self {
        Self {
            granted: ids
                .into_iter()
                .map(|id| crate::environment::CatalogService {
                    id: crate::environment::ServiceId::new(id),
                    name: id.to_string(),
                    description: None,
                })
                .collect(),
            failure: None,
        }
    }

    pub(crate) fn unreachable() -> Self {
        Self {
            granted: Vec::new(),
            failure: Some(CatalogError::Unavailable("connection refused".into())),
        }
    }
}

#[async_trait::async_trait]
impl crate::environment::ServiceCatalog for FakeServiceCatalog {
    async fn available(
        &self,
        _subject: &SubjectId,
        _credential: Option<&str>,
    ) -> Result<Vec<crate::environment::CatalogService>, CatalogError> {
        match &self.failure {
            Some(CatalogError::Unavailable(m)) => Err(CatalogError::Unavailable(m.clone())),
            Some(CatalogError::Malformed(m)) => Err(CatalogError::Malformed(m.clone())),
            None => Ok(self.granted.clone()),
        }
    }
}

/// A profile service wired to a service catalogue, with one mandated service.
pub(crate) fn service_catalog_service(
    catalog: Option<Arc<FakeServiceCatalog>>,
) -> Arc<EnvironmentProfileService> {
    EnvironmentProfileService::new(
        Arc::new(FakeProfileStore::default()),
        Baseline::new(
            [SoftwarePackage::new("platform-cli")],
            [],
            [crate::environment::ServiceId::new("nyxid")],
        ),
        None,
        catalog.map(|c| c as Arc<dyn crate::environment::ServiceCatalog>),
        fake_agent(),
        unconfigured_settings(),
    )
}

/// The baseline the tests treat as what this deployment mandates.
pub(crate) fn baseline() -> Baseline {
    Baseline::new(
        [SoftwarePackage::new("platform-cli")],
        [SkillId::new("mandatory-one"), SkillId::new("mandatory-two")],
        [],
    )
}

/// A profile service over the given store, with the standard baseline.
pub(crate) fn environments(store: Arc<FakeProfileStore>) -> Arc<EnvironmentProfileService> {
    EnvironmentProfileService::new(
        store,
        baseline(),
        None,
        None,
        fake_agent(),
        unconfigured_settings(),
    )
}

/// A profile service wired to a catalogue.
pub(crate) fn catalog_service(catalog: Arc<FakeSkillCatalog>) -> Arc<EnvironmentProfileService> {
    EnvironmentProfileService::new(
        Arc::new(FakeProfileStore::default()),
        baseline(),
        Some(catalog),
        None,
        crate::environment::AgentConfiguration {
            // The kernel the fake adapter is registered under. The default is
            // the one this deployment ships, and a fixture that left it there
            // would be asking for a runtime the test never registered.
            kernel: crate::ids::KernelId::new("fake"),
            ..Default::default()
        },
        unconfigured_settings(),
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
