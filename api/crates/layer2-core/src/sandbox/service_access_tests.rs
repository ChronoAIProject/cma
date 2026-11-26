use crate::profiles::content_digest;
use crate::{ServiceId, SubjectId};

use super::lifecycle_test_support::{checkpoint, sandbox, NOW};
use super::store::{
    SandboxAdmission, SandboxAdmissionName, SandboxAdmissionReplay, SandboxEventStore,
    SandboxStore, SandboxStoreError, SandboxTransaction, SandboxTransactionStore,
};
use super::*;

fn paused() -> Sandbox {
    let mut value = sandbox();
    value.status = SandboxStatus::Paused;
    value.runtime = None;
    value.lifecycle.checkpoints.latest = Some(checkpoint(1, NOW));
    value.revision = 1;
    value.credential.reaches = vec![ServiceId::new("chrono-llm"), ServiceId::new("ornn-api")];
    value.profile.content.nyxid_services = value.credential.reaches.clone();
    value.profile.digest = content_digest(&value.profile.content).unwrap();
    value.validate().unwrap();
    value
}

fn grant(value: &mut Sandbox) -> SandboxServiceGrant {
    value
        .grant_service_access(
            &value.owner.clone(),
            &[ServiceId::new("api-github")],
            NOW.into(),
        )
        .unwrap()
        .unwrap()
}

fn transaction(before: &Sandbox, next: Sandbox, grant: SandboxServiceGrant) -> SandboxTransaction {
    SandboxTransaction::SaveSandboxAndAppend {
        command_revival: None,
        released_runtime: None,
        expected_revision: before.revision,
        sandbox: Box::new(next),
        event_at: NOW.into(),
        event: SandboxEventKind::ServiceAccessGranted { grant },
    }
}

#[test]
fn explicit_service_access_preserves_profile_creation_scope_and_admission_identity() {
    let mut original = paused();
    original.origin = SandboxOrigin::Trigger {
        launch_id: "lch_grant".into(),
    };
    original.status = SandboxStatus::Provisioning;
    let admission = SandboxAdmission::new(
        original.clone(),
        SandboxAdmissionName::Exact(original.name.clone()),
        3,
        NOW.into(),
    )
    .unwrap();
    original.status = SandboxStatus::Paused;
    let mut changed = original.clone();
    let authorization = grant(&mut changed);
    assert_eq!(changed.profile, original.profile);
    assert_eq!(changed.credential.reaches, original.credential.reaches);
    assert_eq!(changed.lifecycle, original.lifecycle);
    assert_eq!(changed.last_activity_at, original.last_activity_at);
    assert_eq!(
        changed.credential.key_generation,
        original.credential.key_generation + 1
    );
    assert_eq!(
        authorization.credential_generation_floor,
        changed.credential.key_generation
    );
    assert!(changed.needs_service_access_credential());
    assert_eq!(
        changed.credential.effective_reaches(),
        ["api-github", "chrono-llm", "ornn-api"].map(ServiceId::new)
    );
    SandboxAdmissionReplay::new(changed, admission.name, admission.admission_fingerprint).unwrap();
}

#[test]
fn service_access_is_sorted_deduplicated_and_replayed_without_mutating_live_state() {
    let mut value = paused();
    let owner = value.owner.clone();
    let added = value
        .grant_service_access(
            &owner,
            &[
                ServiceId::new("z-service"),
                ServiceId::new("api-github"),
                ServiceId::new("api-github"),
                ServiceId::new("ornn-api"),
            ],
            NOW.into(),
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        added.services,
        ["api-github", "z-service"].map(ServiceId::new)
    );
    value.status = SandboxStatus::Active;
    value.runtime = sandbox().runtime;
    let before = value.clone();
    assert_eq!(
        value
            .grant_service_access(&owner, &added.services, NOW.into())
            .unwrap(),
        None
    );
    assert_eq!(value, before);
}

#[test]
fn invalid_or_unsettled_service_access_requests_are_atomic_refusals() {
    for index in 0..13 {
        let mut value = paused();
        let mut owner = value.owner.clone();
        let mut services = vec![ServiceId::new("api-github")];
        let mut at = NOW.to_owned();
        match index {
            0 => owner = SubjectId::new("other-owner"),
            1 => services.clear(),
            2 => services = vec![ServiceId::new("bad\nservice")],
            3 => services = vec![ServiceId::new(" api-github")],
            4 => services = vec![ServiceId::new("x".repeat(257))],
            5 => services = vec![ServiceId::new("api-github"); 33],
            6 => at = "not-a-date".into(),
            7 => at = "2026-09-04T00:00:00Z".into(),
            8 => value.status = SandboxStatus::Active,
            9 => value.work.step = SandboxWorkStep::RunLifecycleOperation,
            10 => value.credential.pending = value.credential.current.clone(),
            11 => value.revision = u64::MAX,
            _ => value.credential.key_generation = u64::MAX,
        }
        let before = value.clone();
        assert!(
            value.grant_service_access(&owner, &services, at).is_err(),
            "case {index}"
        );
        assert_eq!(value, before, "case {index}");
    }
}

#[test]
fn service_access_has_a_cumulative_bound_and_rejects_forged_records() {
    let mut value = paused();
    let owner = value.owner.clone();
    value
        .grant_service_access(
            &owner,
            &(0..32)
                .map(|i| ServiceId::new(format!("service-{i:02}")))
                .collect::<Vec<_>>(),
            NOW.into(),
        )
        .unwrap();
    let before = value.clone();
    assert!(value
        .grant_service_access(&owner, &[ServiceId::new("extra")], NOW.into())
        .is_err());
    assert_eq!(value, before);
    for index in 0..8 {
        let mut invalid = value.clone();
        let authorization = &mut invalid.credential.service_grants[0];
        match index {
            0 => authorization.authorized_by = SubjectId::new("other"),
            1 => authorization.authorized_at = "2026-09-04T00:00:00Z".into(),
            2 => authorization.authorized_at = "2026-09-06T00:00:00Z".into(),
            3 => authorization.services.swap(0, 1),
            4 => authorization.services.clear(),
            5 => authorization.services = vec![ServiceId::new("ornn-api")],
            6 => authorization.credential_generation_floor = 0,
            _ => authorization.credential_generation_floor += 1,
        }
        assert!(invalid.validate().is_err(), "case {index}");
    }
}

#[test]
fn legacy_serialization_omits_empty_service_access_and_roundtrips_new_grants() {
    let mut value = paused();
    let old = serde_json::to_value(&value).unwrap();
    assert!(old["credential"].get("service_grants").is_none());
    assert_eq!(serde_json::from_value::<Sandbox>(old).unwrap(), value);
    grant(&mut value);
    let decoded: Sandbox = serde_json::from_value(serde_json::to_value(&value).unwrap()).unwrap();
    decoded.validate().unwrap();
    assert_eq!(decoded, value);
}

#[tokio::test]
async fn service_access_store_requires_the_exact_owner_event_and_preserves_replay() {
    let store = InMemorySandboxStore::default();
    let original = paused();
    store.create_sandbox(original.clone()).await.unwrap();
    let mut changed = original.clone();
    let authorization = grant(&mut changed);
    assert_eq!(
        store
            .save_sandbox(original.revision, changed.clone())
            .await
            .unwrap_err(),
        SandboxStoreError::Conflict
    );
    for index in 0..5 {
        let mut invalid = changed.clone();
        let mut event = authorization.clone();
        match index {
            0 => invalid.profile.revision += 1,
            1 => invalid.credential.reaches.push(ServiceId::new("implicit")),
            2 => invalid.name = "unrelated change".into(),
            3 => event.authorized_by = SubjectId::new("other"),
            _ => event.services.push(ServiceId::new("extra")),
        }
        assert_eq!(
            store
                .commit(transaction(&original, invalid, event))
                .await
                .unwrap_err(),
            SandboxStoreError::Conflict
        );
        assert_eq!(
            store.get_sandbox(&original.id).await.unwrap(),
            Some(original.clone())
        );
        assert!(store
            .sandbox_events(&original.id, 0, 10)
            .await
            .unwrap()
            .is_empty());
    }
    store
        .commit(transaction(
            &original,
            changed.clone(),
            authorization.clone(),
        ))
        .await
        .unwrap();
    assert_eq!(
        store.get_sandbox(&original.id).await.unwrap(),
        Some(changed.clone())
    );
    assert_eq!(
        store.sandbox_events(&original.id, 0, 10).await.unwrap()[0].kind,
        SandboxEventKind::ServiceAccessGranted {
            grant: authorization.clone()
        }
    );
    assert_eq!(
        store
            .commit(transaction(&original, changed.clone(), authorization))
            .await
            .unwrap_err(),
        SandboxStoreError::Conflict
    );
    let mut stale = changed.clone();
    stale.credential.key_generation -= 1;
    stale.revision += 1;
    assert_eq!(
        store
            .save_sandbox(changed.revision, stale)
            .await
            .unwrap_err(),
        SandboxStoreError::Conflict
    );
    let mut preloaded = changed;
    preloaded.id = crate::SandboxId("sbx_preloaded".into());
    preloaded.revision = 1;
    assert!(store.create_sandbox(preloaded).await.is_err());
}
