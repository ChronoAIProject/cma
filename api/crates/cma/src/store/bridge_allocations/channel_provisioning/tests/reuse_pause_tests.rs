use super::{reuse::fixture::ReuseFixture, *};
use layer2_core::{
    resource_access::{CallerContext, ResourceAccessService, ResourceOwner},
    sandbox::{agent_io::*, control::*, store::*, *},
};

struct Custody;
#[async_trait::async_trait]
impl AgentCommandAdmissionAuthority for Custody {
    async fn command_admission_authority(
        &self,
        _: &CallerContext,
        _: &str,
        _: &ResourceOwner,
    ) -> Result<AgentAuthorizationRevision, CommandAuthorizationError> {
        Ok(AgentAuthorizationRevision {
            authority_id: "original-human-control".into(),
            policy_revision: 1,
            grant_revision: None,
        })
    }
}

async fn control(f: &ReuseFixture, key: &str) -> SandboxControlRecord {
    let resources = f.original.resources.clone();
    let access = Arc::new(ResourceAccessService::new(
        resources.clone(),
        Arc::new(Authority),
    ));
    let service = SandboxControlService::new(resources.clone(), access.clone(), Arc::new(Custody));
    let current = resources.get_sandbox(&f.parent.id).await.unwrap().unwrap();
    let record = service
        .admit(
            &caller(),
            HUMAN_SECRET,
            SandboxControlSubmission {
                sandbox_id: current.id.clone(),
                command: SandboxControl::Pause,
                expected_revision: current.revision,
                idempotency_key: key.into(),
            },
        )
        .await
        .unwrap();
    let proof = access
        .authorize_sandbox_control(&caller(), HUMAN_SECRET, &record)
        .await
        .unwrap();
    resources
        .apply_sandbox_control(&record, proof)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn shared_parent_pause_waits_for_real_child_setup_in_both_control_paths() {
    for shared_workspace in [false, true] {
        let f = ReuseFixture::new(shared_workspace).await;
        let registry = crate::sandbox::channel_startup::test_support::publish_connection(
            f.original.resources.clone(),
            f.original.adapter.clone(),
            &f.sibling.id,
        )
        .await;
        let (admission, authority) = f.proposal("67890").await;
        let mut record = f
            .original
            .allocations
            .reserve(AllocationReservation::new(admission, authority, now()).unwrap())
            .await
            .unwrap();
        let mut held_phases = 0;
        for step in 0..8 {
            let authority = f.authority(&record).await;
            record = f
                .driver()
                .run_one(record, &authority, &f.original.lease)
                .await
                .unwrap();
            if record.state == AllocationState::Ready {
                break;
            }
            let current = f
                .original
                .resources
                .get_sandbox(&f.parent.id)
                .await
                .unwrap()
                .unwrap();
            let events = f
                .original
                .resources
                .sandbox_events(&current.id, 0, 100)
                .await
                .unwrap();
            assert!(matches!(
                f.original
                    .resources
                    .schedule_sandbox_lifecycle(SandboxLifecycleCommand::pause(
                        current.id.clone(),
                        current.owner.clone(),
                        layer2_core::now_rfc3339(),
                        None,
                    ))
                    .await,
                Err(SandboxStoreError::LifecycleBusy {
                    status: SandboxStatus::Active,
                    reason: SandboxLifecycleBlocker::WorkInProgress,
                })
            ));
            let receipt = control(&f, &format!("pause-while-child-setup-{step}")).await;
            assert!(matches!(
                receipt.status,
                SandboxControlStatus::Rejected {
                    reason: SandboxControlFailure::NotEligible,
                    ..
                }
            ));
            assert_eq!(
                f.original.resources.get_sandbox(&current.id).await.unwrap(),
                Some(current.clone())
            );
            assert_eq!(
                f.original
                    .resources
                    .sandbox_events(&current.id, 0, 100)
                    .await
                    .unwrap(),
                events
            );
            assert_eq!(
                f.original
                    .resources
                    .schedule_image_rotation(SandboxImageRotationRequest {
                        sandbox_id: current.id.clone(),
                        requested_at: layer2_core::now_rfc3339(),
                        expected_runtime_version: "fixture-next-runtime".into(),
                    })
                    .await
                    .unwrap(),
                SandboxMaintenanceOutcome::NotEligible
            );
            held_phases += 1;
            let authority = f.authority(&record).await;
            if let Some(claim) = record.lease.clone() {
                record = f
                    .original
                    .allocations
                    .mutate(
                        AllocationMutation::progress(
                            record,
                            authority,
                            &claim,
                            AllocationProgress::NeedsConnection,
                            now(),
                        )
                        .unwrap(),
                    )
                    .await
                    .unwrap();
            }
        }
        assert!(held_phases > 0);
        assert_eq!(record.state, AllocationState::Ready);
        let receipt = control(&f, "pause-after-child-ready").await;
        assert!(matches!(
            receipt.status,
            SandboxControlStatus::Applied {
                disposition: SandboxControlDisposition::Scheduled,
                ..
            }
        ));
        assert_eq!(
            f.original
                .resources
                .get_sandbox(&f.parent.id)
                .await
                .unwrap()
                .unwrap()
                .status,
            SandboxStatus::Pausing
        );
        let replay = f
            .original
            .resources
            .sandbox_control(&receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(replay, receipt);
        drop(registry);
        f.original.close().await;
    }
}
