use super::*;
use crate::sandbox_usage::Allocation;
use crate::Authorization;

struct Store(SandboxUsage);
#[async_trait]
impl SandboxUsageStore for Store {
    async fn usage(&self, _: &SandboxId) -> Result<Option<SandboxUsage>, SandboxStoreError> {
        Ok(Some(self.0.clone()))
    }
    async fn usage_page(
        &self,
        _: Option<&SubjectId>,
        _: &PageRequest,
    ) -> Result<Page<SandboxUsage>, SandboxStoreError> {
        unreachable!()
    }
    async fn usage_summary(
        &self,
        _: Option<&SubjectId>,
        _: u64,
    ) -> Result<UsageSummary, SandboxStoreError> {
        unreachable!()
    }
    async fn usage_events(
        &self,
        _: &SandboxId,
        _: &PageRequest,
    ) -> Result<Page<UsageEvent>, SandboxStoreError> {
        unreachable!()
    }
}
fn caller(subject: &str, capabilities: &[Capability]) -> Caller {
    Caller {
        subject: SubjectId(subject.into()),
        authorization: Authorization::new(capabilities.iter().copied()),
    }
}
#[test]
fn usage_scope_is_owner_bound_and_admin_scope_requires_explicit_authority() {
    let owner = caller("owner", &[Capability::UseOwnSandboxes]);
    assert_eq!(scope(&owner, false, None).unwrap(), Some(&owner.subject));
    assert!(scope(&owner, false, Some(&SubjectId("someone-else".into()))).is_err());
    assert!(scope(&owner, true, None).is_err());
    let admin = caller("operator", &[Capability::ReadAllSandboxes]);
    assert_eq!(scope(&admin, true, None).unwrap(), None);
    assert_eq!(
        scope(&admin, true, Some(&owner.subject)).unwrap(),
        Some(&owner.subject)
    );
    assert!(scope(&caller("owner", &[]), false, None).is_err());
    assert!(scope(
        &caller("visitor", &[Capability::LaunchTriggers]),
        false,
        None
    )
    .is_ok());
}
#[tokio::test]
async fn retained_usage_is_readable_only_by_owner_or_operator() {
    let state = SandboxUsage {
        sandbox_id: SandboxId("retired-sandbox".into()),
        owner: SubjectId("owner".into()),
        name: "Research".into(),
        status: "retired".into(),
        allocation: Allocation::default(),
        created_at: 1,
        tracking_started_at: 1,
        last_observed_at: 2,
        source_seq: 2,
        source_revision: 2,
        revision: 2,
        has_started: true,
        complete_lifecycle: true,
        totals: UsageTotals::default(),
    };
    let service = SandboxUsageService::new(Arc::new(Store(state.clone())));
    assert!(service
        .read(
            &caller("owner", &[Capability::UseOwnSandboxes]),
            &state.sandbox_id
        )
        .await
        .is_ok());
    assert!(service
        .read(
            &caller("admin", &[Capability::ReadAllSandboxes]),
            &state.sandbox_id
        )
        .await
        .is_ok());
    for actor in [
        caller("other", &[Capability::UseOwnSandboxes]),
        caller("owner", &[]),
    ] {
        assert!(matches!(
            service.read(&actor, &state.sandbox_id).await,
            Err(UsageReadError::NotFound)
        ));
        assert!(matches!(
            service
                .events(&actor, &state.sandbox_id, &PageRequest::default())
                .await,
            Err(UsageReadError::NotFound)
        ));
    }
}
