use std::sync::Arc;

use async_trait::async_trait;
use layer2_core::profiles::{
    ProfileAdminQuery, ProfileAdminStore, ProfileAdminSummary, ProfileLocator, ProfilePage,
    ProfileStoreError,
};
use layer2_core::sandbox::{
    InMemorySandboxStore, OperatorError, SandboxOperatorService, SandboxRetirement,
    SandboxRetirementResult,
};
use layer2_core::{
    ActivityService, Authorization, Capability, PlatformSettings, PlatformSettingsService,
    PlatformSettingsStore, SandboxId, StoreError, StoredPlatformSettings,
};

use super::super::super::admin::{router, AdminState};
use super::super::Scenario;
use super::settings_store::UnavailableActivity;

struct ConflictingSettings(bool);

#[async_trait]
impl PlatformSettingsStore for ConflictingSettings {
    async fn read(&self) -> Result<StoredPlatformSettings, StoreError> {
        let mut settings = PlatformSettings::default();
        settings.triggers.orchestration_paused = self.0;
        Ok(StoredPlatformSettings {
            revision: 0,
            settings,
        })
    }

    async fn replace(
        &self,
        expected: u64,
        _: &PlatformSettings,
    ) -> Result<StoredPlatformSettings, StoreError> {
        Err(StoreError::RevisionConflict {
            expected,
            actual: expected + 1,
        })
    }
}

struct ConflictingRetirement;

struct ConflictingProfileRetirement;

#[async_trait]
impl ProfileAdminStore for ConflictingProfileRetirement {
    async fn list_profiles(
        &self,
        _: &ProfileAdminQuery,
    ) -> Result<ProfilePage<ProfileAdminSummary>, ProfileStoreError> {
        panic!("the retirement conflict scenario does not query inventory")
    }

    async fn retire_profile(
        &self,
        _: &ProfileLocator,
        _: String,
    ) -> Result<ProfileAdminSummary, ProfileStoreError> {
        Err(ProfileStoreError::Conflict)
    }
}

#[async_trait]
impl SandboxRetirement for ConflictingRetirement {
    async fn retire(
        &self,
        _: &SandboxId,
        _: String,
        _: Option<u64>,
    ) -> Result<SandboxRetirementResult, OperatorError> {
        Err(OperatorError::InvalidState(
            "contract retirement revision changed".into(),
        ))
    }
}

pub(super) fn scenario(name: &str) -> Option<Scenario> {
    if !matches!(name, "operator_conflict" | "operator_resume_conflict") {
        return None;
    }
    let activity = ActivityService::new(Arc::new(UnavailableActivity));
    let settings = Arc::new(PlatformSettingsService::new(
        Arc::new(ConflictingSettings(name == "operator_resume_conflict")),
        activity.clone(),
    ));
    let operators = Arc::new(SandboxOperatorService::new(
        Arc::new(InMemorySandboxStore::default()),
        Arc::new(ConflictingProfileRetirement),
        Arc::new(ConflictingRetirement),
        activity.clone(),
    ));
    Some(Scenario {
        router: router(AdminState::new(settings, operators, activity)),
        subject: Some("operator-a"),
        authorization: Authorization::new([
            Capability::ConfigurePlatform,
            Capability::ControlAnySandbox,
        ]),
        symbols: Default::default(),
    })
}
