use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Notify;

use super::*;
use crate::{SandboxProfileId, SubjectId};

fn bounds() -> ProfileCapacityBounds {
    ProfileCapacityBounds {
        min_cpu_millis: 100,
        max_cpu_millis: 8000,
        min_memory_mib: 128,
        max_memory_mib: 16384,
    }
}

fn profile(visibility: ProfileVisibility) -> SandboxProfile {
    SandboxProfile {
        availability: Default::default(),
        id: SandboxProfileId("sbp_values".into()),
        owner: SubjectId("owner".into()),
        name: "Values".into(),
        description: String::new(),
        visibility,
        forked_from: None,
        uses_count: 0,
        draft_version: 1,
        created_at: "2026-09-07T00:00:00Z".into(),
        updated_at: "2026-09-07T00:00:00Z".into(),
        retired_at: None,
        draft: SandboxProfileContent {
            runtime_image: None,
            cpu_millis: 1000,
            memory_mib: 1024,
            nyxid_services: vec![],
            agent_provider: AgentProvider::default(),
            model_access: ModelAccess::ChronoaiManaged,
            max_workspaces: 4,
            max_agents_per_workspace: 4,
            workspace_profiles: vec![],
            software_install_commands: vec![],
            environment_variables: vec![EnvironmentVariable {
                name: "CONFIGURATION".into(),
                value: "plain-value".into(),
            }],
        },
    }
}

#[derive(Debug)]
struct Unavailable;
#[async_trait]
impl PublicValueScanner for Unavailable {
    async fn scan(&self, _: &[EnvironmentVariable]) -> Result<(), PublicValueScanError> {
        Err(PublicValueScanError::Unavailable)
    }
}

#[derive(Debug, Default)]
struct Paused {
    started: Notify,
    release: Notify,
}
#[async_trait]
impl PublicValueScanner for Paused {
    async fn scan(&self, _: &[EnvironmentVariable]) -> Result<(), PublicValueScanError> {
        self.started.notify_one();
        self.release.notified().await;
        Ok(())
    }
}

#[tokio::test]
async fn public_publication_fails_closed_and_returns_only_the_variable_name() {
    let store = InMemoryProfileStore::default();
    let value = profile(ProfileVisibility::Public);
    store.create_sandbox_profile(value.clone()).await.unwrap();
    let refusal = store
        .publish_sandbox_profile(
            &value.id,
            1,
            value.updated_at.clone(),
            bounds(),
            &Unavailable,
        )
        .await
        .unwrap_err();
    assert!(
        matches!(refusal, ProfileStoreError::Validation(error) if error.code == "public_value_scanner_unavailable")
    );
    let refusal = store
        .publish_sandbox_profile(
            &value.id,
            1,
            value.updated_at.clone(),
            bounds(),
            &["plain-value".into()],
        )
        .await
        .unwrap_err();
    assert_eq!(
        refusal,
        ProfileStoreError::Validation(ProfileError::environment_secret("CONFIGURATION"))
    );
    assert!(!format!("{refusal}").contains("plain-value"));
    assert!(store
        .sandbox_profile_revision(&value.id, 1)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn visibility_scans_every_immutable_revision_and_does_not_change_private_content() {
    let store = InMemoryProfileStore::default();
    let mut value = profile(ProfileVisibility::Private);
    store.create_sandbox_profile(value.clone()).await.unwrap();
    store
        .publish_sandbox_profile(
            &value.id,
            1,
            value.updated_at.clone(),
            bounds(),
            &Unavailable,
        )
        .await
        .unwrap();
    value.draft_version = 2;
    value.draft.environment_variables[0].value = "new-value".into();
    store.save_sandbox_draft(1, value.clone()).await.unwrap();
    store
        .publish_sandbox_profile(
            &value.id,
            2,
            value.updated_at.clone(),
            bounds(),
            &Unavailable,
        )
        .await
        .unwrap();
    let locator = MarketplaceLocator::Sandbox(value.id.clone());
    let refusal = store
        .set_profile_visibility(
            &value.owner,
            &locator,
            ProfileVisibility::Public,
            value.updated_at.clone(),
            bounds(),
            &["plain-value".into()],
        )
        .await
        .unwrap_err();
    assert_eq!(
        refusal,
        MarketplaceError::PublicValues(ProfileError::environment_secret("CONFIGURATION"))
    );
    assert!(
        matches!(store.set_profile_visibility(&value.owner, &locator, ProfileVisibility::Public, value.updated_at.clone(), bounds(), &Unavailable).await, Err(MarketplaceError::PublicValues(error)) if error.code == "public_value_scanner_unavailable")
    );
    assert_eq!(
        store.sandbox_profile(&value.id).await.unwrap().unwrap(),
        value
    );
}

#[tokio::test]
async fn publication_rechecks_the_snapshot_after_scanning_without_holding_the_store_lock() {
    let store = InMemoryProfileStore::default();
    let mut value = profile(ProfileVisibility::Public);
    store.create_sandbox_profile(value.clone()).await.unwrap();
    let scanner = Arc::new(Paused::default());
    let task = {
        let store = store.clone();
        let scanner = scanner.clone();
        let value = value.clone();
        tokio::spawn(async move {
            store
                .publish_sandbox_profile(&value.id, 1, value.updated_at, bounds(), scanner.as_ref())
                .await
        })
    };
    scanner.started.notified().await;
    value.draft_version = 2;
    value.draft.environment_variables[0].value = "concurrent-value".into();
    store.save_sandbox_draft(1, value.clone()).await.unwrap();
    scanner.release.notify_one();
    assert_eq!(task.await.unwrap(), Err(ProfileStoreError::Conflict));
    assert!(store
        .sandbox_profile_revision(&value.id, 1)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn visibility_rechecks_revision_inventory_after_scanning() {
    let store = InMemoryProfileStore::default();
    let value = profile(ProfileVisibility::Private);
    store.create_sandbox_profile(value.clone()).await.unwrap();
    store
        .publish_sandbox_profile(
            &value.id,
            1,
            value.updated_at.clone(),
            bounds(),
            &Unavailable,
        )
        .await
        .unwrap();
    let scanner = Arc::new(Paused::default());
    let task = {
        let store = store.clone();
        let scanner = scanner.clone();
        let value = value.clone();
        tokio::spawn(async move {
            store
                .set_profile_visibility(
                    &value.owner,
                    &MarketplaceLocator::Sandbox(value.id),
                    ProfileVisibility::Public,
                    value.updated_at,
                    bounds(),
                    scanner.as_ref(),
                )
                .await
        })
    };
    scanner.started.notified().await;
    store
        .publish_sandbox_profile(
            &value.id,
            1,
            value.updated_at.clone(),
            bounds(),
            &Unavailable,
        )
        .await
        .unwrap();
    scanner.release.notify_one();
    assert!(matches!(
        task.await.unwrap(),
        Err(MarketplaceError::Unavailable(_))
    ));
    assert_eq!(
        store
            .sandbox_profile(&value.id)
            .await
            .unwrap()
            .unwrap()
            .visibility,
        ProfileVisibility::Private
    );
}
