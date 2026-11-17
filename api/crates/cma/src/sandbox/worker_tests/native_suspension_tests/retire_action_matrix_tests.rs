use super::*;
use layer2_core::sandbox::{
    SandboxLifecycleAction, SandboxLifecycleResult, SandboxRotationIdentity,
};
use layer3_adapter::agent_sandbox::NativePoolControl;

#[tokio::test]
async fn advertised_retire_matches_owner_delete_for_every_status_and_operation_kind() {
    use SandboxStatus::*;
    let statuses = [
        Provisioning,
        Installing,
        Active,
        Pausing,
        Paused,
        Reviving,
        CredentialRotating,
        Rotating,
        Failed,
        Retiring,
        Retired,
    ];
    for status in statuses {
        for kind in ["none", "pause", "revive", "image_rotation"] {
            let mut f = Fixture::native().await;
            let created = f.harness.create().await;
            let mut now = start_time(&created);
            let active = f
                .drive(&created.id, &mut now, Active, &runtime_image())
                .await;
            f.harness.adapter = f.adapter(&runtime_image());
            crate::sandbox::runtime_attachment::adopt_bound_runtime(
                f.harness.adapter.as_ref(),
                &active,
                "test",
            )
            .await
            .unwrap();
            let mut candidate = active.clone();
            match kind {
                "pause" => candidate.begin_pause(rfc3339_at(now), None).unwrap(),
                "revive" => {
                    candidate = f.pause(&active, &mut now).await;
                    candidate
                        .begin_revive(SandboxReviveCause::ExplicitRequest, rfc3339_at(now))
                        .unwrap();
                }
                "image_rotation" => {
                    let identity = SandboxRotationIdentity::capture(&candidate, &[]).unwrap();
                    candidate
                        .begin_image_rotation("image-v2".into(), identity, rfc3339_at(now))
                        .unwrap();
                }
                _ => {}
            }
            candidate.status = status;
            candidate.failure = (status == Failed).then(|| layer2_core::sandbox::TypedFailure {
                class: "restore_failed".into(),
                detail: "Reopen rejected".into(),
            });
            if status == Paused {
                candidate.runtime = None;
            }
            if status == Retired {
                candidate.retired_at = Some(rfc3339_at(now));
            }
            let valid = candidate.validate().is_ok();
            let result =
                SandboxLifecycleResult::observe(&candidate, Some(&rfc3339_at(now)), now, true);
            let has_result = result.is_some();
            let advertised =
                result.is_some_and(|value| value.actions.contains(&SandboxLifecycleAction::Retire));
            let mut proposed = candidate.clone();
            let admitted = valid
                && proposed.begin_retirement(rfc3339_at(now)).is_ok()
                && proposed.validate().is_ok();
            assert_eq!(advertised, has_result && admitted, "{status:?}/{kind}");
            if !admitted {
                assert!(!advertised, "{status:?}/{kind}");
                continue;
            }
            if !advertised {
                continue;
            } // No lifecycle result on a settled state.
            let current = f.harness.stored(&active.id).await;
            candidate.revision = current.revision + 1;
            f.harness
                .store
                .save_sandbox(current.revision, candidate.clone())
                .await
                .unwrap();
            let owner = caller("owner", [Capability::DeleteOwnSandbox]);
            let deleting = f
                .harness
                .service
                .delete(&owner, &candidate.id)
                .await
                .unwrap();
            assert_eq!(deleting.status, Retiring, "{status:?}/{kind}");
            assert!(deleting.lifecycle.operation.is_none());
            assert!(deleting.work.lease.is_none());
            let retired = f
                .drive(&candidate.id, &mut now, Retired, &runtime_image())
                .await;
            assert!(retired.work.lease.is_none());
            assert!(retired.runtime.is_none());
            assert!(retired.lifecycle.home.as_ref().unwrap().released);
            assert!(f
                .adapter(&runtime_image())
                .runtime_inventory()
                .await
                .unwrap()
                .is_empty());
            assert!(f
                .backend
                .pools
                .as_ref()
                .unwrap()
                .inventory_claims()
                .await
                .unwrap()
                .is_empty());
            assert_eq!(
                f.harness
                    .service
                    .delete(&owner, &candidate.id)
                    .await
                    .unwrap(),
                retired
            );
        }
    }
}
