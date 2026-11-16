//! A slow admission read cannot preserve authority revoked before receipt replay.

use super::*;
use crate::admission::OperatorStop;
use crate::budget::{
    BudgetError, Closure, ClosureReason, CloudObservationInput, ObservationError,
    OperatorAdmissionState, PlatformAdmissionState, PlatformSpendLedger, PlatformSpendState,
};

struct RevokeDuringRead {
    access: Arc<ResourceAccessService>,
    revoke: Mutex<Option<ShareCommand>>,
    stopped: PlatformAdmissionState,
}

#[async_trait::async_trait]
impl PlatformSpendLedger for RevokeDuringRead {
    async fn admission_state(&self, _: u64) -> Result<PlatformAdmissionState, BudgetError> {
        let revoke = self.revoke.lock().unwrap().take().unwrap();
        self.access
            .change_share(&context("owner"), Some("fixture"), revoke)
            .await
            .unwrap();
        Ok(self.stopped.clone())
    }

    async fn spend_state(&self, _: u64) -> Result<PlatformSpendState, BudgetError> {
        unreachable!("receipt recovery reads only admission state")
    }

    async fn record_cloud_observation(
        &self,
        _: CloudObservationInput,
        _: u64,
    ) -> Result<PlatformSpendState, ObservationError> {
        unreachable!("receipt recovery records no observation")
    }
}

async fn replay_rechecks_policy(operator_stop: bool) {
    let f = Fixture::new().await;
    let share = f.share(ShareMode::OrgRoles, None).await;
    let revision = f.store.lock().sandboxes[&sandbox(4).id].revision;
    f.admit("recipient", SandboxControl::Revive, "revive-race", revision)
        .await
        .unwrap();
    let before = f.store.lock().clone();
    let stopped = if operator_stop {
        PlatformAdmissionState {
            operator_control: OperatorAdmissionState::Stopped(OperatorStop {
                since: 1,
                revision: 1,
                actor: SubjectId::new("operator"),
                reason: "contain new work".into(),
            }),
            ..Default::default()
        }
    } else {
        PlatformAdmissionState {
            configured: true,
            closures: vec![Closure {
                reason: ClosureReason::SpendStop,
                since: 1,
                settings_revision: 1,
                committed_at_entry: 0,
            }],
            ..Default::default()
        }
    };
    f.service.attach_platform_spend(Arc::new(RevokeDuringRead {
        access: f.access.clone(),
        revoke: Mutex::new(Some(ShareCommand {
            selector: share.share.selector,
            change: ShareChange::Revoke {
                share_id: share.share.id,
            },
            expected_share_revision: Some(share.share.revision),
            idempotency_key: "revoke-during-admission".into(),
        })),
        stopped,
    }));
    let result = f
        .admit("recipient", SandboxControl::Revive, "revive-race", revision)
        .await;
    assert!(
        matches!(
            result,
            Err(SandboxControlServiceError::Store(
                SandboxControlError::Conflict
            ))
        ),
        "receipt recovery must recheck the revoked policy: {result:?}"
    );
    let after = f.store.lock().clone();
    assert_eq!(after.sandbox_controls, before.sandbox_controls);
    assert_eq!(after.sandboxes, before.sandboxes);
    assert_eq!(after.sandbox_events, before.sandbox_events);
}

#[tokio::test]
async fn operator_stop_replay_rechecks_policy_after_admission_read() {
    replay_rechecks_policy(true).await;
}

#[tokio::test]
async fn spend_stop_replay_rechecks_policy_after_admission_read() {
    replay_rechecks_policy(false).await;
}
