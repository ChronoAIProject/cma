use crate::{ServiceId, SubjectId};

use super::lifecycle_test_support::{sandbox, NOW};
use super::store::{SandboxEventStore, SandboxStore, SandboxTransaction, SandboxTransactionStore};
use super::*;

fn services(names: &[&str]) -> Vec<ServiceId> {
    names.iter().map(|name| ServiceId::new(*name)).collect()
}

fn live() -> Sandbox {
    let mut value = sandbox();
    value.revision = 1;
    value.runtime.as_mut().unwrap().provision_generation = Some(1);
    value.lifecycle.runtime_generation_high_water = 1;
    value.credential.reaches = services(&["baseline", "repository"]);
    value.work = SandboxWorkState::new(
        SandboxWorkStep::WaitForCredentialRotation,
        "2026-09-06T00:00:00Z".into(),
    );
    value.validate().unwrap();
    value
}

fn delivery_completed(value: &mut Sandbox) {
    value.credential.key_generation += 1;
    value.credential.current.as_mut().unwrap().generation = value.credential.key_generation;
    value.validate().unwrap();
}

#[test]
fn exact_live_scope_removes_creation_and_added_services_without_changing_runtime() {
    let before = live();
    let mut value = before.clone();
    let scope = value
        .revise_live_service_access(
            &before.owner,
            &services(&["baseline", "calendar"]),
            NOW.into(),
        )
        .unwrap();
    assert_eq!(scope.revision, 1);
    assert_eq!(scope.runtime, before.runtime.clone().unwrap());
    assert_eq!(
        scope.runtime_services,
        before.credential.effective_reaches()
    );
    assert_eq!(
        value
            .credential
            .runtime_reaches(value.runtime.as_ref().unwrap()),
        before.credential.effective_reaches()
    );
    assert_eq!(
        value.credential.effective_reaches(),
        services(&["baseline", "calendar"])
    );
    assert_eq!(value.credential.reaches, before.credential.reaches);
    assert_eq!(value.credential.current, before.credential.current);
    assert_eq!(value.credential.delivered, before.credential.delivered);
    assert_eq!(
        value.credential.service_grants,
        before.credential.service_grants
    );
    assert_eq!(
        value.credential.key_generation,
        before.credential.key_generation + 1
    );
    assert_eq!(
        value.credential.service_access_generation_floor(),
        scope.credential_generation_floor
    );
    assert!(value.needs_service_access_credential());
    assert_eq!(value.runtime, before.runtime);
    assert_eq!(value.profile, before.profile);
    assert_eq!(value.lifecycle, before.lifecycle);
    assert_eq!(value.last_activity_at, before.last_activity_at);
    assert_eq!(value.status, SandboxStatus::Active);
    assert!(value.work.is_due(NOW));
    assert_eq!(value.work.step, SandboxWorkStep::WaitForCredentialRotation);
}

#[test]
fn repeated_exact_scope_still_reserves_a_new_delivery_and_can_regrant_removed_service() {
    let mut value = live();
    let owner = value.owner.clone();
    let first = value
        .revise_live_service_access(&owner, &services(&["baseline"]), NOW.into())
        .unwrap();
    let pending = value.clone();
    assert!(value
        .revise_live_service_access(&owner, &services(&["repository"]), NOW.into())
        .is_err());
    assert_eq!(value, pending);
    delivery_completed(&mut value);
    let second = value
        .revise_live_service_access(&owner, &services(&["baseline"]), NOW.into())
        .unwrap();
    assert_eq!(second.revision, first.revision + 1);
    assert!(second.credential_generation_floor > first.credential_generation_floor);
    delivery_completed(&mut value);
    let third = value
        .revise_live_service_access(&owner, &services(&["repository", "baseline"]), NOW.into())
        .unwrap();
    assert_eq!(third.services, services(&["baseline", "repository"]));
    assert_eq!(third.runtime_services, first.runtime_services);
    assert_eq!(value.credential.effective_reaches(), third.services);
    assert!(value
        .grant_service_access(&owner, &services(&["implicit"]), NOW.into())
        .is_err());
}

#[test]
fn startup_scope_follows_runtime_generation_and_survives_a_process_boot_rebind() {
    let mut value = live();
    value.runtime.as_mut().unwrap().provision_generation = Some(4);
    value.lifecycle.runtime_generation_high_water = 4;
    let owner = value.owner.clone();
    let original = value.runtime.clone().unwrap();
    let first = value
        .revise_live_service_access(&owner, &services(&["calendar"]), NOW.into())
        .unwrap();
    delivery_completed(&mut value);
    let mut restarted_process = original.clone();
    restarted_process.boot_id = Some("b".repeat(32));
    assert_eq!(
        value.credential.runtime_reaches(&restarted_process),
        first.runtime_services
    );
    let second = value
        .revise_live_service_access(&owner, &services(&["calendar", "mail"]), NOW.into())
        .unwrap();
    assert_eq!(second.runtime_services, first.runtime_services);
    delivery_completed(&mut value);

    // An explicit lifecycle resume renders the current exact scope in the new
    // generation, even when the provider retains its original runtime handle.
    let mut resumed = original.clone();
    resumed.provision_generation = Some(5);
    assert_eq!(
        value.credential.runtime_reaches(&resumed),
        services(&["calendar", "mail"])
    );
    value.runtime = Some(resumed.clone());
    value.lifecycle.runtime_generation_high_water = 5;
    let third = value
        .revise_live_service_access(&owner, &services(&["mail"]), NOW.into())
        .unwrap();
    assert_eq!(third.runtime, resumed);
    assert_eq!(third.runtime_services, services(&["calendar", "mail"]));
    assert_eq!(value.credential.effective_reaches(), services(&["mail"]));
}

#[test]
fn live_revision_refuses_unsettled_or_foreign_authority_atomically() {
    for case in 0..15 {
        let mut value = live();
        let mut owner = value.owner.clone();
        let mut desired = services(&["baseline"]);
        match case {
            0 => owner = SubjectId::new("foreign"),
            1 => desired.clear(),
            2 => desired = services(&["baseline", "baseline"]),
            3 => desired = services(&["invalid scope"]),
            4 => value.status = SandboxStatus::Paused,
            5 => value.runtime = None,
            6 => value.work.step = SandboxWorkStep::MintRotatedCredential,
            7 => {
                value.work.lease = Some(SandboxWorkLease {
                    holder: "other".into(),
                    fence: 1,
                    expires_at: "2026-09-06T00:00:00Z".into(),
                })
            }
            8 => value.credential.pending = value.credential.current.clone(),
            9 => value.credential.revoke_debt.push(SandboxRevokeDebt {
                provider_key_id: "old".into(),
                queued_at: NOW.into(),
            }),
            10 => value.credential.key_generation = u64::MAX,
            11 => value.credential.key_generation = u64::MAX - 1,
            12 => value.revision = u64::MAX,
            13 => {
                desired = (0..33)
                    .map(|index| ServiceId::new(format!("service-{index}")))
                    .collect()
            }
            _ => value.runtime.as_mut().unwrap().provision_generation = None,
        }
        let before = value.clone();
        assert!(
            value
                .revise_live_service_access(&owner, &desired, NOW.into())
                .is_err(),
            "case {case}"
        );
        assert_eq!(value, before, "case {case}");
    }
}

#[test]
fn legacy_scope_deserialization_stays_additive_and_new_scope_survives_round_trip() {
    let mut value = live();
    let legacy = serde_json::to_value(&value).unwrap();
    assert!(legacy["credential"].get("service_scope").is_none());
    assert_eq!(serde_json::from_value::<Sandbox>(legacy).unwrap(), value);
    value
        .revise_live_service_access(&value.owner.clone(), &services(&["baseline"]), NOW.into())
        .unwrap();
    let wire = serde_json::to_value(&value).unwrap();
    let round_trip: Sandbox = serde_json::from_value(wire).unwrap();
    round_trip.validate().unwrap();
    assert_eq!(round_trip, value);
    for case in 0..9 {
        let mut invalid = value.clone();
        let scope = invalid.credential.service_scope.as_mut().unwrap();
        match case {
            0 => scope.authorized_by = SubjectId::new("foreign"),
            1 => scope.services.clear(),
            2 => scope.revision = 0,
            3 => scope.credential_generation_floor = 0,
            4 => scope.credential_generation_floor = value.credential.key_generation + 1,
            5 => scope.runtime.runtime_id.clear(),
            6 => scope.runtime.provision_generation = Some(0),
            7 => scope.runtime_services = services(&["duplicate", "duplicate"]),
            _ => scope.runtime.bound_at = "2026-09-07T00:00:00Z".into(),
        }
        assert!(invalid.validate().is_err());
    }
}

#[tokio::test]
async fn exact_scope_revision_requires_matching_durable_event_and_preserves_legacy_grants() {
    let store = InMemorySandboxStore::default();
    let before = live();
    store.create_sandbox(before.clone()).await.unwrap();
    let mut next = before.clone();
    let scope = next
        .revise_live_service_access(&before.owner, &services(&["baseline"]), NOW.into())
        .unwrap();
    assert!(store
        .save_sandbox(before.revision, next.clone())
        .await
        .is_err());
    let event = SandboxEventKind::ServiceAccessRevised {
        scope: Box::new(scope.clone()),
    };
    let mut forged = next.clone();
    forged.runtime.as_mut().unwrap().runtime_id = "different-runtime".into();
    assert!(before
        .validate_service_access_transaction(&forged, &event, NOW)
        .is_err());
    store
        .commit(SandboxTransaction::SaveSandboxAndAppend {
            command_revival: None,
            released_runtime: None,
            expected_revision: before.revision,
            sandbox: Box::new(next.clone()),
            event_at: NOW.into(),
            event: event.clone(),
        })
        .await
        .unwrap();
    assert_eq!(store.get_sandbox(&before.id).await.unwrap(), Some(next));
    assert_eq!(
        store.sandbox_events(&before.id, 0, 10).await.unwrap()[0].kind,
        event
    );
    let mut preloaded = store.get_sandbox(&before.id).await.unwrap().unwrap();
    preloaded.id = crate::SandboxId("sbx_preloaded_scope".into());
    preloaded.revision = 1;
    assert!(store.create_sandbox(preloaded).await.is_err());
}
