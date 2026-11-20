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
