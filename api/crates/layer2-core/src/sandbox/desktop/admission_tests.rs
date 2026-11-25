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
    for (status, bound) in [
        (SandboxStatus::Active, true),
        (SandboxStatus::Paused, false),
        (SandboxStatus::Failed, true),
        (SandboxStatus::Retiring, true),
        (SandboxStatus::Retired, false),
    ] {
        let mut root = desktop_sandbox(true, status, bound);
        match status {
            SandboxStatus::Paused => {
                root.lifecycle.checkpoints.latest =
                    Some(crate::sandbox::lifecycle_test_support::checkpoint(1, NOW));
            }
            SandboxStatus::Failed => {
                root.failure = Some(TypedFailure {
                    class: "fixture_failure".into(),
                    detail: "Synthetic lifecycle failure".into(),
                });
            }
            SandboxStatus::Retired => root.retired_at = Some(NOW.into()),
            _ => {}
        }
        root.validate()
            .unwrap_or_else(|error| panic!("invalid {status:?} desktop fixture: {error}"));
        let service = desktop_service(root, false, AgentStatus::Ready)
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
