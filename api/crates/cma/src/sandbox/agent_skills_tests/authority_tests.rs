use super::*;
use crate::auth::persistence::SecretValue;
use crate::sandbox::credential::{CredentialError, CredentialMaterial};
use layer2_core::profiles::{
    AgentProvider, ModelAccess, SandboxProfileContent, SandboxProfileSnapshot,
};
use layer2_core::sandbox::{
    SandboxCredentialAuthority, SandboxCredentialState, SandboxLimits, SandboxOrigin,
    SandboxSignInState, SandboxStatus, SandboxWorkState, SandboxWorkStep,
};
use layer2_core::{SandboxId, SandboxProfileId, ServiceId, SubjectId};
use std::time::SystemTime;

pub(super) fn sandbox() -> Sandbox {
    let stamp = "2026-09-05T00:00:00Z";
    Sandbox {
        id: SandboxId("sbx_skill_owner".into()),
        owner: SubjectId::new("owner-a"),
        name: "skill-sandbox".into(),
        origin: SandboxOrigin::Manual,
        profile: SandboxProfileSnapshot {
            id: SandboxProfileId("sbp_skill_owner".into()),
            revision: 1,
            digest: "a".repeat(64),
            always_on: None,
            content: SandboxProfileContent {
                runtime_image: None,
                cpu_millis: 1000,
                memory_mib: 2048,
                nyxid_services: vec![ServiceId::new("ornn-api")],
                agent_provider: AgentProvider::Codex,
                model_access: ModelAccess::ChronoaiManaged,
                max_workspaces: 1,
                max_agents_per_workspace: 1,
                workspace_profiles: vec![],
                software_install_commands: vec![],
                environment_variables: vec![],
            },
        },
        status: SandboxStatus::Active,
        failure: None,
        runtime: None,
        credential: SandboxCredentialState {
            source: Default::default(),
            service_grants: Vec::new(),
            service_scope: None,
            reaches: vec![ServiceId::new("ornn-api")],
            key_generation: 3,
            delivered: true,
            current: Some(SandboxCredentialAuthority {
                generation: 3,
                provider_key_id: "sandbox-key-3".into(),
                provider_state_version: 3,
                issued_at: stamp.into(),
                expires_at: "2026-10-05T00:00:00Z".into(),
                grant_fingerprint: "b".repeat(64),
            }),
            pending: None,
            revoke_debt: vec![],
            sign_in: SandboxSignInState::NotRequired,
        },
        work: SandboxWorkState::new(SandboxWorkStep::None, stamp.into()),
        limits: SandboxLimits {
            max_workspaces: 1,
            max_agents_per_workspace: 1,
        },
        lifecycle: Default::default(),
        last_activity_at: stamp.into(),
        created_at: stamp.into(),
        updated_at: stamp.into(),
        retired_at: None,
        purge_after: None,
        revision: 1,
    }
}

fn credential_material() -> CredentialMaterial {
    let sandbox = sandbox();
    CredentialMaterial {
        source: Default::default(),
        github_route: None,
        sandbox_id: sandbox.id,
        owner: sandbox.owner,
        authority: sandbox.credential.current.unwrap(),
        value: SecretValue::new(TOKEN.into()),
    }
}

pub(super) enum CredentialReply {
    Present(Box<CredentialMaterial>),
    Missing,
    Failed(String),
    Refused(String),
    Pending,
}

pub(super) struct TestCredentials {
    reply: CredentialReply,
    pub(super) calls: AtomicUsize,
    requested: Mutex<Vec<(SandboxId, u64)>>,
}

impl TestCredentials {
    pub(super) fn present() -> Arc<Self> {
        Self::with(CredentialReply::Present(Box::new(credential_material())))
    }
    pub(super) fn with(reply: CredentialReply) -> Arc<Self> {
        Arc::new(Self {
            reply,
            calls: AtomicUsize::new(0),
            requested: Mutex::new(vec![]),
        })
    }
}

#[async_trait::async_trait]
impl SandboxCredentialController for TestCredentials {
    async fn material(
        &self,
        sandbox_id: &SandboxId,
        generation: u64,
    ) -> Result<Option<CredentialMaterial>, CredentialError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.requested
            .lock()
            .unwrap()
            .push((sandbox_id.clone(), generation));
        match &self.reply {
            CredentialReply::Present(material) => Ok(Some(material.as_ref().clone())),
            CredentialReply::Missing => Ok(None),
            CredentialReply::Failed(error) => Err(CredentialError::Unavailable(error.clone())),
            CredentialReply::Refused(error) => Err(CredentialError::Security(error.clone())),
            CredentialReply::Pending => std::future::pending().await,
        }
    }
    async fn issue(
        &self,
        _: &Sandbox,
        _: u64,
        _: SystemTime,
    ) -> Result<CredentialMaterial, CredentialError> {
        unreachable!("skill resolution must not mint authority")
    }
    async fn custodied(&self, _: &SandboxId) -> Result<Vec<CredentialMaterial>, CredentialError> {
        unreachable!("skill resolution must not enumerate authority")
    }
    async fn forget(&self, _: &SandboxId, _: u64) -> Result<(), CredentialError> {
        unreachable!("skill resolution must not delete authority")
    }
    async fn revoke(&self, _: &Sandbox, _: &str) -> Result<(), CredentialError> {
        unreachable!("skill resolution must not revoke authority")
    }
}

#[tokio::test]
async fn public_initial_selection_uses_current_scoped_custody_without_browser_authority() {
    let registry = Registry::start().await;
    registry.reply(
        "skl_public",
        "",
        Reply::json(material("skl_public", "4.0", &"d".repeat(64))),
    );
    let credentials = TestCredentials::present();
    let resolver = resolver(&registry, Some(credentials.clone()));
    let sandbox = sandbox();
    let selected = resolver
        .resolve(&sandbox, &[reference("skl_public", None)])
        .await
        .unwrap();
    assert_eq!(selected[0].version, "4.0");
    assert_eq!(*credentials.requested.lock().unwrap(), [(sandbox.id, 3)]);
    assert_eq!(
        registry.requests()[0].authorization.as_deref(),
        Some("Bearer admitted-sandbox-key")
    );
    resolver.runtime_skills(&selected).unwrap();
    resolver.runtime_skills(&selected).unwrap();
    assert_eq!(credentials.calls.load(Ordering::Relaxed), 1);
    assert_eq!(registry.requests().len(), 1);
    assert!(!serde_json::to_string(&selected).unwrap().contains(TOKEN));
}

#[tokio::test]
async fn mismatched_scoped_custody_fails_before_any_metadata_request() {
    for case in 0..10 {
        let registry = Registry::start().await;
        let mut sandbox = sandbox();
        let mut material = credential_material();
        match case {
            0 => material.owner = SubjectId::new("other-owner"),
            1 => material.sandbox_id = SandboxId("sbx_other".into()),
            2 => material.authority.generation = 2,
            3 => material.authority.provider_key_id = "different-key".into(),
            4 => material.authority.grant_fingerprint = "c".repeat(64),
            5 => sandbox.credential.reaches.clear(),
            6 => sandbox.credential.delivered = false,
            7 => material.value = SecretValue::new(" ".into()),
            8 => material.authority.provider_state_version = 4,
            _ => sandbox.credential.current = None,
        }
        let credentials = TestCredentials::with(CredentialReply::Present(Box::new(material)));
        let error = resolver(&registry, Some(credentials.clone()))
            .resolve(&sandbox, &[reference("skl_public", None)])
            .await
            .unwrap_err();
        assert!(
            matches!(error, AgentSkillResolutionError::Invalid(_)),
            "case {case}: {error}"
        );
        assert!(!error.to_string().contains(TOKEN));
        assert!(registry.requests().is_empty());
        assert_eq!(
            credentials.calls.load(Ordering::Relaxed),
            usize::from(!matches!(case, 5 | 6 | 9))
        );
    }
}

#[tokio::test]
async fn pending_rotation_still_uses_exact_delivered_current_generation() {
    let registry = Registry::start().await;
    registry.reply(
        "skl_public",
        "1.0",
        Reply::json(material("skl_public", "1.0", &"d".repeat(64))),
    );
    let credentials = TestCredentials::present();
    let mut sandbox = sandbox();
    sandbox.credential.key_generation = 4;
    let mut pending = sandbox.credential.current.clone().unwrap();
    pending.generation = 4;
    pending.provider_key_id = "pending-key-4".into();
    sandbox.credential.pending = Some(pending);
    resolver(&registry, Some(credentials.clone()))
        .resolve(&sandbox, &[reference("skl_public", Some("1.0"))])
        .await
        .unwrap();
    assert_eq!(*credentials.requested.lock().unwrap(), [(sandbox.id, 3)]);
}

#[tokio::test]
async fn refused_custody_does_not_leak_key_diagnostics() {
    let registry = Registry::start().await;
    let credentials = TestCredentials::with(CredentialReply::Refused(format!("refused {TOKEN}")));
    let error = resolver(&registry, Some(credentials))
        .resolve(&sandbox(), &[reference("skl_public", None)])
        .await
        .unwrap_err();
    assert!(matches!(error, AgentSkillResolutionError::Invalid(_)));
    assert!(!error.to_string().contains(TOKEN));
    assert!(registry.requests().is_empty());
}
