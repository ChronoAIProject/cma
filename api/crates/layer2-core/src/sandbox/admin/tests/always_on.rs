use super::*;
use crate::sandbox::store::SandboxMaintenanceStore;

#[tokio::test]
async fn always_on_requires_control_and_audits_approval_reapproval_and_revocation() {
    let f = fixture();
    let original = sandbox("sbx_exception", "owner-a", SandboxStatus::Active, "v1");
    f.sandboxes.create_sandbox(original.clone()).await.unwrap();
    let admin = caller([Capability::ControlAnySandbox]);
    let owner = caller([Capability::UseOwnSandboxes]);
    assert_eq!(
        f.service
            .set_always_on(&owner, &original.id, original.revision, true)
            .await,
        Err(OperatorError::Forbidden)
    );
    assert!(f.activity.records.lock().unwrap().is_empty());
    assert_eq!(
        f.sandboxes.get_sandbox(&original.id).await.unwrap(),
        Some(original.clone())
    );
    let mut revision = original.revision;
    for value in [true, true, false] {
        let saved = f
            .service
            .set_always_on(&admin, &original.id, revision, value)
            .await
            .unwrap();
        assert_eq!(saved.revision, revision + 1);
        assert_eq!(saved.profile.always_on, Some(value));
        let records = f.activity.records.lock().unwrap();
        let record = records.last().unwrap();
        assert_eq!(record.actor, Some(admin.subject.clone()));
        assert_eq!(record.action, ActivityAction::Configure);
        assert_eq!(record.outcome, ActivityOutcome::Succeeded);
        assert_eq!(
            record.target,
            Some(ActivityTarget::Resource(format!("sandbox:{}", original.id)))
        );
        assert_eq!(record.detail.get(DetailKey::Setting), Some("always_on"));
        assert_eq!(
            record.detail.get(DetailKey::Status),
            Some(value.to_string().as_str())
        );
        assert_eq!(
            record.detail.get(DetailKey::Revision),
            Some(saved.revision.to_string().as_str())
        );
        revision = saved.revision;
    }
    assert!(matches!(
        f.service
            .set_always_on(&admin, &original.id, original.revision, true)
            .await,
        Err(OperatorError::InvalidState(_))
    ));
    assert_eq!(f.activity.records.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn always_on_filter_pages_only_explicit_exceptions_and_tracks_revocation() {
    let f = fixture();
    for (id, preference) in [
        ("sbx_a", Some(true)),
        ("sbx_b", None),
        ("sbx_c", Some(false)),
        ("sbx_d", Some(true)),
    ] {
        let mut value = sandbox(id, "owner-a", SandboxStatus::Active, "v1");
        value.profile.always_on = preference;
        f.sandboxes.create_sandbox(value).await.unwrap();
    }
    let admin = caller([Capability::ReadAllSandboxes]);
    let mut query = SandboxAdminQuery {
        always_on: Some(true),
        page: PageRequest::new(1, None).unwrap(),
        ..Default::default()
    };
    let first = f.service.list_sandboxes(&admin, &query).await.unwrap();
    assert_eq!(first.items[0].id.as_str(), "sbx_a");
    query.page.cursor = first.next;
    let second = f.service.list_sandboxes(&admin, &query).await.unwrap();
    assert_eq!(second.items[0].id.as_str(), "sbx_d");
    assert!(second.next.is_none());
    f.sandboxes
        .set_sandbox_always_on(second.items[0].id.clone(), 1, false)
        .await
        .unwrap();
    query.page = PageRequest::default();
    assert_eq!(
        f.service
            .list_sandboxes(&admin, &query)
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    query.always_on = Some(false);
    assert_eq!(
        f.service
            .list_sandboxes(&admin, &query)
            .await
            .unwrap()
            .items
            .len(),
        3
    );
}

#[tokio::test]
async fn unconfigured_maintenance_is_unavailable_only_after_authorization() {
    let mut f = fixture();
    f.service.maintenance = None;
    let id = SandboxId("sbx_unconfigured".into());
    assert!(matches!(
        f.service
            .set_always_on(&caller([Capability::ControlAnySandbox]), &id, 1, true)
            .await,
        Err(OperatorError::Unavailable(_))
    ));
    assert_eq!(
        f.service
            .set_always_on(&caller([Capability::UseOwnSandboxes]), &id, 1, true)
            .await,
        Err(OperatorError::Forbidden)
    );
    assert!(f.activity.records.lock().unwrap().is_empty());
}

struct FailingActivity;

#[async_trait::async_trait]
impl ActivityStore for FailingActivity {
    async fn append(&self, _record: &ActivityRecord) -> Result<(), StoreError> {
        Err(StoreError::Unavailable("activity unavailable".into()))
    }

    async fn list(
        &self,
        _query: &ActivityQuery,
    ) -> Result<ActivityPage<ActivityRecord>, StoreError> {
        Err(StoreError::Unavailable("activity unavailable".into()))
    }
}

#[tokio::test]
async fn audit_failure_matches_retirement_write_then_append_contract() {
    let mut f = fixture();
    let original = sandbox("sbx_audit_failure", "owner-a", SandboxStatus::Active, "v1");
    f.sandboxes.create_sandbox(original.clone()).await.unwrap();
    f.service.activity = ActivityService::new(Arc::new(FailingActivity));
    let admin = caller([Capability::ControlAnySandbox]);
    let result = f
        .service
        .set_always_on(&admin, &original.id, original.revision, true)
        .await;
    assert!(matches!(result, Err(OperatorError::Unavailable(_))));
    let changed = f
        .sandboxes
        .get_sandbox(&original.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(changed.profile.always_on, Some(true));
    assert_eq!(changed.revision, original.revision + 1);
    // As for retirement, audit failure cannot be reported as success or undo the write.
    // A fresh read and explicit re-approval establishes a confirmed audited revision.
    f.service.activity = ActivityService::new(f.activity.clone());
    let confirmed = f
        .service
        .set_always_on(&admin, &original.id, changed.revision, true)
        .await
        .unwrap();
    let records = f.activity.records.lock().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].detail.get(DetailKey::Revision),
        Some(confirmed.revision.to_string().as_str())
    );
}
