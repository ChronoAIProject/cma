use super::*;

struct CurrentAuthority;

#[async_trait::async_trait]
impl DesktopAuthorization for CurrentAuthority {
    async fn current(&self, _: &Caller) -> Result<(), DesktopError> {
        Ok(())
    }
}

#[tokio::test]
async fn default_and_disabled_services_refuse_discovery_and_both_open_paths() {
    let caller = Caller::new(
        SubjectId("owner".into()),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let id = AgentId("agt_test".into());
    for explicit in [false, true] {
        let service =
            desktop_fixture(true, false, SandboxStatus::Active, true, AgentStatus::Ready).await;
        let service = if explicit {
            service.with_admission(DesktopAdmission::Disabled)
        } else {
            service
        };
        assert!(matches!(
            service.available(&caller, &id).await,
            Err(DesktopError::AuthorityUnavailable)
        ));
        assert!(matches!(
            service.open(&caller, &id).await,
            Err(DesktopError::AuthorityUnavailable)
        ));
        assert!(matches!(
            service
                .open_authorized(&caller, &id, Arc::new(CurrentAuthority))
                .await,
            Err(DesktopError::AuthorityUnavailable)
        ));
    }
}

#[tokio::test]
async fn protected_admission_requires_current_authority_and_supported_receiver() {
    let service = desktop_fixture(true, false, SandboxStatus::Active, true, AgentStatus::Ready)
        .await
        .with_admission(DesktopAdmission::Protected);
    let caller = Caller::new(
        SubjectId("owner".into()),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let id = AgentId("agt_test".into());
    assert!(service.available(&caller, &id).await.is_ok());
    assert!(matches!(
        service.open(&caller, &id).await,
        Err(DesktopError::AuthorityUnavailable)
    ));
    assert!(matches!(
        service
            .open_authorized(&caller, &id, Arc::new(CurrentAuthority))
            .await,
        Err(DesktopError::RuntimeUnavailable)
    ));
}

#[tokio::test]
async fn desktop_work_refuses_inactive_sandboxes_before_receiver_challenge() {
    let caller = Caller::new(
        SubjectId("owner".into()),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let id = AgentId("agt_test".into());
    for status in [
        SandboxStatus::Active,
        SandboxStatus::Paused,
        SandboxStatus::Failed,
        SandboxStatus::Retiring,
        SandboxStatus::Retired,
    ] {
        let service = desktop_fixture(true, false, status, true, AgentStatus::Ready)
            .await
            .with_admission(DesktopAdmission::Protected);
        let result = service
            .open_authorized(&caller, &id, Arc::new(CurrentAuthority))
            .await;
        if status == SandboxStatus::Active {
            // This fixture's receiver challenge reports unsupported.
            assert!(matches!(result, Err(DesktopError::RuntimeUnavailable)));
        } else {
            assert!(
                matches!(result, Err(DesktopError::Unavailable)),
                "{status:?} must be refused before reaching the receiver",
            );
        }
    }
}
