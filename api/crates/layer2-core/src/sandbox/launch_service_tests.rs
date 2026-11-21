use std::sync::Arc;

use crate::profiles::store::InMemoryProfileStore;
use crate::{Authorization, Caller, Capability, SandboxId, ServiceId, SubjectId};

use super::lifecycle_test_support::{checkpoint, sandbox};
use super::store::{SandboxAdmission, SandboxAdmissionName, SandboxAdmissionStore, SandboxStore};
use super::*;

const CREATED_AT: &str = "2026-09-05T00:00:00Z";
const BOUND_AT: &str = "2026-09-05T00:01:00Z";
const FAILED_AT: &str = "2026-09-05T00:02:00Z";
type SandboxCorruption = fn(&mut Sandbox);

#[tokio::test]
async fn launch_authorities_repair_only_the_exact_post_install_failure() {
    let (service, store) = fixture();
    for (suffix, capability) in [
        ("launch", Capability::LaunchTriggers),
        ("create", Capability::CreateSandbox),
    ] {
        let failed = admit_failed(&store, suffix, &format!("launch-{suffix}"), |_| {}).await;
        let runtime = failed.runtime.clone();
        let identity = (
            failed.id.clone(),
            failed.owner.clone(),
            failed.name.clone(),
            failed.origin.clone(),
            failed.profile.clone(),
            failed.credential.clone(),
            failed.lifecycle.clone(),
        );
        let outcome = service
            .repair_service_grant_for_launch(
                &caller("owner", [capability]),
                &failed.id,
                &format!("launch-{suffix}"),
            )
            .await
            .unwrap();

        assert_eq!(
            outcome.disposition,
            SandboxLaunchCommandDisposition::Scheduled
        );
        assert_eq!(outcome.sandbox.status, SandboxStatus::Installing);
        assert_eq!(outcome.sandbox.failure, None);
        assert_eq!(outcome.sandbox.runtime, runtime);
        assert_eq!(
            outcome.sandbox.work.step,
            SandboxWorkStep::MintInitialCredential
        );
        assert_eq!(outcome.sandbox.work.attempts, 0);
        assert_eq!(outcome.sandbox.work.lease, None);
        assert_eq!(outcome.sandbox.revision, failed.revision + 1);
        assert_eq!(
            (
                outcome.sandbox.id.clone(),
                outcome.sandbox.owner.clone(),
                outcome.sandbox.name.clone(),
                outcome.sandbox.origin.clone(),
                outcome.sandbox.profile.clone(),
                outcome.sandbox.credential.clone(),
                outcome.sandbox.lifecycle.clone(),
            ),
            identity
        );
    }
}

#[tokio::test]
async fn launch_repair_preserves_legacy_unknown_runtime_generation() {
    let (service, store) = fixture();
    let failed = admit_failed(&store, "legacy", "launch-legacy", |value| {
        value.runtime.as_mut().unwrap().provision_generation = None;
        value.lifecycle.runtime_generation_high_water = 0;
    })
    .await;
    let repaired = service
        .repair_service_grant_for_launch(
            &caller("owner", [Capability::LaunchTriggers]),
            &failed.id,
            "launch-legacy",
        )
        .await
        .unwrap();
    assert_eq!(
        repaired.disposition,
        SandboxLaunchCommandDisposition::Scheduled
    );
    assert_eq!(repaired.sandbox.runtime, failed.runtime);
    assert_eq!(repaired.sandbox.lifecycle.runtime_generation_high_water, 0);
}

#[tokio::test]
async fn launch_repair_authorizes_first_and_hides_foreign_sandboxes() {
    let (service, store) = fixture();
    let failed = admit_failed(&store, "authority", "launch-authority", |_| {}).await;
    assert_eq!(
        service
            .repair_service_grant_for_launch(
                &caller("owner", []),
                &SandboxId("sbx_missing".into()),
                "",
            )
            .await,
        Err(SandboxServiceError::Forbidden)
    );
    assert_eq!(
        service
            .repair_service_grant_for_launch(
                &caller("other", [Capability::LaunchTriggers]),
                &failed.id,
                "launch-authority",
            )
            .await,
        Err(SandboxServiceError::NotFound)
    );
    for launch_id in ["", "   ", "launch\ncontrol"] {
        assert_eq!(
            service
                .repair_service_grant_for_launch(
                    &caller("owner", [Capability::LaunchTriggers]),
                    &failed.id,
                    launch_id,
                )
                .await,
            Err(SandboxServiceError::InvalidOrigin)
        );
    }
    assert_eq!(
        service
            .repair_service_grant_for_launch(
                &caller("owner", [Capability::LaunchTriggers]),
                &failed.id,
                "launch-other",
            )
            .await,
        Err(SandboxServiceError::SandboxOriginConflict)
    );
    assert_eq!(store.get_sandbox(&failed.id).await.unwrap(), Some(failed));
}

#[tokio::test]
async fn launch_repair_rejects_every_non_worker_failure_shape() {
    let (service, store) = fixture();
    let corruptions: [(&str, SandboxCorruption); 8] = [
        ("class", |value| {
            value.failure.as_mut().unwrap().class = "terminal_security".into();
        }),
        ("detail", |value| {
            value.failure.as_mut().unwrap().detail = "calendar".into();
        }),
        ("runtime", |value| value.runtime = None),
        ("credential", |value| {
            value.credential.sign_in = SandboxSignInState::Complete;
        }),
        ("work", |value| {
            value.work.step = SandboxWorkStep::RunInstall;
        }),
        ("lifecycle", |value| value.lifecycle.next_authority = 2),
        ("replacement", |value| {
            value.runtime.as_mut().unwrap().provision_generation = Some(2);
            value.lifecycle.runtime_generation_high_water = 2;
        }),
        ("highwater", |value| {
            value.lifecycle.runtime_generation_high_water = 2
        }),
    ];
    for (suffix, corrupt) in corruptions {
        let launch_id = format!("launch-invalid-{suffix}");
        let failed = admit_failed(&store, &format!("invalid-{suffix}"), &launch_id, corrupt).await;
        assert_eq!(
            service
                .repair_service_grant_for_launch(
                    &caller("owner", [Capability::LaunchTriggers]),
                    &failed.id,
                    &launch_id,
                )
                .await,
            Err(SandboxServiceError::InvalidState),
            "accepted {suffix} corruption"
        );
        assert_eq!(store.get_sandbox(&failed.id).await.unwrap(), Some(failed));
    }
}

#[tokio::test]
async fn launch_repair_replays_only_post_install_progress_and_active_completion() {
    let (service, store) = fixture();
    let failed = admit_failed(&store, "replay", "launch-replay", |_| {}).await;
    let owner = caller("owner", [Capability::LaunchTriggers]);
    let scheduled = service
        .repair_service_grant_for_launch(&owner, &failed.id, "launch-replay")
        .await
        .unwrap();
    let replay = service
        .repair_service_grant_for_launch(&owner, &failed.id, "launch-replay")
        .await
        .unwrap();
    assert_eq!(
        replay.disposition,
        SandboxLaunchCommandDisposition::AlreadyScheduled
    );
    assert_eq!(replay.sandbox, scheduled.sandbox);

    let mut active = scheduled.sandbox;
    let ready = sandbox();
    active.status = SandboxStatus::Active;
    active.credential = ready.credential;
    active.credential.reaches = vec![ServiceId::new("github")];
    active
        .work
        .schedule(SandboxWorkStep::WaitForCredentialRotation, FAILED_AT.into());
    active.updated_at = FAILED_AT.into();
    active.revision += 1;
    store
        .save_sandbox(active.revision - 1, active.clone())
        .await
        .unwrap();
    let complete = service
        .repair_service_grant_for_launch(&owner, &active.id, "launch-replay")
        .await
        .unwrap();
    assert_eq!(
        complete.disposition,
        SandboxLaunchCommandDisposition::AlreadySatisfied
    );
    assert_eq!(complete.sandbox, active);
}

#[tokio::test]
async fn launch_revive_binds_created_and_reused_origins_and_uses_trigger_cause() {
    run_launch_revival(false).await;
}

#[tokio::test]
async fn operator_stop_refuses_owner_revival_but_allows_admitted_launch_revival_once() {
    run_launch_revival(true).await;
}

async fn run_launch_revival(stopped: bool) {
    let (service, store) = fixture();
    let owner = caller(
        "owner",
        [Capability::CreateSandbox, Capability::UseOwnSandboxes],
    );
    let created = admit_paused(&store, "created", "launch-created").await;
    if stopped {
        use crate::admission::AdmissionControlStore;
        service.attach_platform_spend(store.clone());
        store
            .change(
                0,
                &SubjectId::new("operator"),
                "contain new work",
                true,
                100,
            )
            .await
            .unwrap();
        assert_eq!(
            service.revive(&owner, &created.id).await,
            Err(SandboxLifecycleCommandError::AdmissionPaused)
        );
    }
    let outcome = service
        .revive_for_launch(&owner, &created.id, "launch-created", false)
        .await
        .unwrap();
    assert_eq!(
        outcome.disposition,
        SandboxLaunchCommandDisposition::Scheduled
    );
    assert_eq!(outcome.sandbox.status, SandboxStatus::Reviving);
    assert!(matches!(
        outcome.sandbox.lifecycle.operation,
        Some(SandboxLifecycleOperation::Revive {
            cause: SandboxReviveCause::TriggerLaunch,
            ..
        })
    ));
    let replay = service
        .revive_for_launch(&owner, &created.id, "launch-created", false)
        .await
        .unwrap();
    assert_eq!(
        replay.disposition,
        SandboxLaunchCommandDisposition::AlreadyScheduled
    );

    let manual = manual_paused("manual");
    store.create_sandbox(manual.clone()).await.unwrap();
    assert_eq!(
        service
            .revive_for_launch(&owner, &manual.id, "launch-reusing-manual", true)
            .await
            .unwrap()
            .disposition,
        SandboxLaunchCommandDisposition::Scheduled
    );
    let older = admit_paused(&store, "older", "launch-older").await;
    assert_eq!(
        service
            .revive_for_launch(&owner, &older.id, "launch-new", true)
            .await
            .unwrap()
            .disposition,
        SandboxLaunchCommandDisposition::Scheduled
    );
}

#[tokio::test]
async fn launch_revive_rejects_wrong_origin_without_broadening_public_revive() {
    let (service, store) = fixture();
    let paused = admit_paused(&store, "wrong-origin", "launch-old").await;
    let launcher = caller("owner", [Capability::LaunchTriggers]);
    assert_eq!(
        service
            .revive_for_launch(&launcher, &paused.id, "launch-new", false)
            .await,
        Err(SandboxServiceError::SandboxOriginConflict)
    );
    assert_eq!(
        service.revive(&launcher, &paused.id).await,
        Err(SandboxLifecycleCommandError::Forbidden)
    );
    assert_eq!(store.get_sandbox(&paused.id).await.unwrap(), Some(paused));
}

#[tokio::test]
async fn launch_revive_treats_an_active_owned_sandbox_as_satisfied() {
    let (service, store) = fixture();
    let mut active = sandbox();
    active.id = SandboxId("sbx_launch_active_reuse".into());
    active.name = "launch-active-reuse".into();
    active.credential.reaches = vec![ServiceId::new("github")];
    active.work =
        SandboxWorkState::new(SandboxWorkStep::WaitForCredentialRotation, FAILED_AT.into());
    active.revision = 1;
    store.create_sandbox(active.clone()).await.unwrap();

    let outcome = service
        .revive_for_launch(
            &caller("owner", [Capability::LaunchTriggers]),
            &active.id,
            "launch-active-reuse",
            true,
        )
        .await
        .unwrap();
    assert_eq!(
        outcome.disposition,
        SandboxLaunchCommandDisposition::AlreadySatisfied
    );
    assert_eq!(outcome.sandbox, active);
    assert_eq!(store.get_sandbox(&active.id).await.unwrap(), Some(active));
}

fn fixture() -> (Arc<SandboxService>, Arc<InMemorySandboxStore>) {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let store = Arc::new(InMemorySandboxStore::default());
    let service = SandboxService::new(
        store.clone(),
        profiles,
        SandboxPolicy::default(),
        Arc::new(crate::sandbox::FixedSandboxCredentialPrerequisite::Ready),
    );
    (service, store)
}

fn caller(subject: &str, capabilities: impl IntoIterator<Item = Capability>) -> Caller {
    Caller::new(SubjectId::new(subject), Authorization::new(capabilities))
}

async fn admit_failed(
    store: &InMemorySandboxStore,
    suffix: &str,
    launch_id: &str,
    mutate: fn(&mut Sandbox),
) -> Sandbox {
    let fresh = fresh_launch_sandbox(suffix, launch_id);
    admit(store, fresh.clone()).await;
    let mut failed = fresh;
    failed.status = SandboxStatus::Failed;
    failed.failure = Some(TypedFailure {
        class: "service_not_granted".into(),
        detail: "github".into(),
    });
    failed.runtime = Some(RuntimeBinding {
        boot_id: None,
        runtime_id: format!("runtime-{suffix}"),
        runtime_version: "image-v1".into(),
        provision_generation: Some(1),
        bound_at: BOUND_AT.into(),
    });
    failed.lifecycle.runtime_generation_high_water = 1;
    failed.work = SandboxWorkState::new(SandboxWorkStep::None, FAILED_AT.into());
    failed.updated_at = FAILED_AT.into();
    failed.revision = 2;
    mutate(&mut failed);
    failed.validate().unwrap();
    store.save_sandbox(1, failed.clone()).await.unwrap();
    failed
}

async fn admit_paused(store: &InMemorySandboxStore, suffix: &str, launch_id: &str) -> Sandbox {
    let fresh = fresh_launch_sandbox(suffix, launch_id);
    admit(store, fresh.clone()).await;
    let mut paused = fresh;
    paused.status = SandboxStatus::Paused;
    let reaches = paused.credential.reaches.clone();
    paused.credential = sandbox().credential;
    paused.credential.reaches = reaches;
    paused.work = SandboxWorkState::new(SandboxWorkStep::None, FAILED_AT.into());
    paused.lifecycle.checkpoints.latest = Some(checkpoint(1, BOUND_AT));
    paused.updated_at = FAILED_AT.into();
    paused.revision = 2;
    paused.validate().unwrap();
    store.save_sandbox(1, paused.clone()).await.unwrap();
    paused
}

async fn admit(store: &InMemorySandboxStore, sandbox: Sandbox) {
    let name = sandbox.name.clone();
    store
        .admit_sandbox(
            SandboxAdmission::new(
                sandbox,
                SandboxAdmissionName::Exact(name),
                100,
                CREATED_AT.into(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
}

fn fresh_launch_sandbox(suffix: &str, launch_id: &str) -> Sandbox {
    let mut value = sandbox();
    value.id = SandboxId(format!("sbx_launch_{suffix}"));
    value.name = format!("launch-{suffix}");
    value.origin = SandboxOrigin::Trigger {
        launch_id: launch_id.into(),
    };
    value.profile.content.nyxid_services = vec![ServiceId::new("github")];
    value.status = SandboxStatus::Provisioning;
    value.failure = None;
    value.runtime = None;
    value.credential.reaches = vec![ServiceId::new("github")];
    value.credential.key_generation = 0;
    value.credential.delivered = false;
    value.credential.current = None;
    value.credential.pending = None;
    value.credential.revoke_debt.clear();
    value.work = SandboxWorkState::new(SandboxWorkStep::ProvisionRuntime, CREATED_AT.into());
    value.lifecycle = Default::default();
    value.last_activity_at = CREATED_AT.into();
    value.created_at = CREATED_AT.into();
    value.updated_at = CREATED_AT.into();
    value.revision = 1;
    value
}

fn manual_paused(suffix: &str) -> Sandbox {
    let mut value = sandbox();
    value.id = SandboxId(format!("sbx_launch_{suffix}"));
    value.name = format!("launch-{suffix}");
    value.status = SandboxStatus::Paused;
    value.runtime = None;
    value.work = SandboxWorkState::new(SandboxWorkStep::None, FAILED_AT.into());
    value.lifecycle.checkpoints.latest = Some(checkpoint(2, BOUND_AT));
    value.revision = 1;
    value
}
