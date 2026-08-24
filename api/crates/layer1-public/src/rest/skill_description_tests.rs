//! Session skill-description projection tests.

use std::sync::Arc;

use tokio::sync::Mutex;

use super::*;
use layer2_core::{
    AgentConfiguration, Authorization, Baseline, Capability, CatalogError, CatalogSkill,
    EnvironmentProfile, EnvironmentProfileId, EnvironmentProfileStore, GrantedSkill, SkillCatalog,
    SkillId, StoreError, SubjectId, UserDirectory, UserDirectoryStore,
};

struct EmptyProfiles;

#[async_trait::async_trait]
impl EnvironmentProfileStore for EmptyProfiles {
    async fn upsert(&self, _profile: &EnvironmentProfile) -> Result<(), StoreError> {
        Ok(())
    }

    async fn get(
        &self,
        _id: &EnvironmentProfileId,
    ) -> Result<Option<EnvironmentProfile>, StoreError> {
        Ok(None)
    }

    async fn list_for(&self, _owner: &SubjectId) -> Result<Vec<EnvironmentProfile>, StoreError> {
        Ok(Vec::new())
    }

    async fn delete(&self, _id: &EnvironmentProfileId) -> Result<(), StoreError> {
        Ok(())
    }
}

struct EmptyDirectory;

#[async_trait::async_trait]
impl UserDirectoryStore for EmptyDirectory {
    async fn record_sign_in(&self, _user: &layer2_core::RegisteredUser) -> Result<(), StoreError> {
        Ok(())
    }

    async fn recent(
        &self,
        _query: &layer2_core::DirectoryQuery,
    ) -> Result<Vec<layer2_core::RegisteredUser>, StoreError> {
        Ok(Vec::new())
    }

    async fn by_subjects(
        &self,
        _subjects: &[SubjectId],
    ) -> Result<Vec<layer2_core::RegisteredUser>, StoreError> {
        Ok(Vec::new())
    }
}

#[derive(Default)]
struct TargetedDescriptions {
    calls: Mutex<Vec<(Option<String>, Vec<String>)>>,
}

#[async_trait::async_trait]
impl SkillCatalog for TargetedDescriptions {
    async fn available(
        &self,
        _subject: &SubjectId,
        _credential: Option<&str>,
    ) -> Result<Vec<CatalogSkill>, CatalogError> {
        panic!("session rendering must not walk the full picker catalogue")
    }

    async fn describe(
        &self,
        _subject: &SubjectId,
        credential: Option<&str>,
        skills: &[SkillId],
    ) -> Result<Vec<CatalogSkill>, CatalogError> {
        self.calls.lock().await.push((
            credential.map(str::to_string),
            skills.iter().map(|skill| skill.0.clone()).collect(),
        ));
        // Return in a different order to prove projection matches by identity,
        // while the request order itself remains deterministic.
        Ok(skills
            .iter()
            .rev()
            .map(|skill| CatalogSkill {
                id: SkillId::new(format!("guid-{}", skill.0)),
                name: skill.0.clone(),
                description: format!("Description for {}", skill.0),
                access: None,
            })
            .collect())
    }
}

fn service(catalog: Arc<TargetedDescriptions>) -> Arc<EnvironmentProfileService> {
    EnvironmentProfileService::new(
        Arc::new(EmptyProfiles),
        Baseline::new([], [], []),
        Some(catalog),
        None,
        AgentConfiguration::default(),
        Arc::new(layer2_core::PlatformSettingsService::unconfigured()),
    )
}

fn caller() -> Caller {
    Caller::new(
        SubjectId("subject-a".into()),
        Authorization::from_capabilities([Capability::ManageOwnEnvironmentProfiles]),
    )
}

fn session(id: &str, skills: &[&str]) -> Session {
    Session {
        id: SessionId(id.into()),
        owner: SubjectId("subject-a".into()),
        kernel: layer2_core::KernelId::new("codex"),
        status: layer2_core::SessionStatus::Ready,
        recovery_state: layer2_core::RecoveryState::Normal,
        stop_reason: None,
        options: layer2_core::AgentOptions::default(),
        environment_profile: None,
        geometry: TerminalGeometry::default(),
        last_seq: 0,
        runtime_version: None,
        rotations: 0,
        rotated_at: None,
        capabilities: Default::default(),
        granted_services: None,
        granted_skills: Some(
            skills
                .iter()
                .map(|skill| GrantedSkill {
                    id: SkillId::new(*skill),
                    version: Some("0.5".into()),
                })
                .collect(),
        ),
        pricing: None,
        created_at: "2026-08-24T00:00:00Z".into(),
    }
}

#[tokio::test]
async fn out_of_page_grants_gain_descriptions_through_one_deduplicated_targeted_call() {
    let catalog = Arc::new(TargetedDescriptions::default());
    let environments = service(Arc::clone(&catalog));
    let sessions = vec![
        session("sesn_one", &["outside-first-page"]),
        session("sesn_two", &["outside-first-page", "second-skill"]),
    ];

    let views = viewed(
        &environments,
        &UserDirectory::new(Arc::new(EmptyDirectory)),
        &caller(),
        Some("delegated-token"),
        sessions,
    )
    .await;

    let calls = catalog.calls.lock().await;
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0.as_deref(), Some("delegated-token"));
    assert_eq!(
        calls[0].1,
        vec!["outside-first-page".to_string(), "second-skill".to_string()],
        "first appearance determines one stable request order"
    );
    drop(calls);

    let first = views[0].agent.skills.as_ref().expect("recorded skills");
    assert_eq!(first[0].name.as_deref(), Some("outside-first-page"));
    assert_eq!(
        first[0].description.as_deref(),
        Some("Description for outside-first-page")
    );
    assert_eq!(first[0].version.as_deref(), Some("0.5"));
    let second = views[1].agent.skills.as_ref().expect("recorded skills");
    assert_eq!(
        second[1].description.as_deref(),
        Some("Description for second-skill")
    );
}
