use super::*;
use crate::sandbox::agent_io::ResourceCommandAuthorizationError;
use crate::sandbox::{SandboxCredentialPrerequisite, SandboxCredentialPrerequisiteError};
use std::sync::atomic::{AtomicUsize, Ordering};

struct WorkerAuthority {
    actor: String,
    calls: AtomicUsize,
    state: Mutex<Result<(), CommandAuthorizationError>>,
}
#[async_trait::async_trait]
impl SandboxControlAuthorizer for WorkerAuthority {
    async fn authorize_sandbox_control(
        &self,
        access: &ResourceAccessService,
        record: &SandboxControlRecord,
    ) -> Result<Arc<SandboxControlAuthorization>, ResourceCommandAuthorizationError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.state.lock().unwrap().clone()?;
        Ok(access
            .authorize_sandbox_control(&context(&self.actor), "fixture", record)
            .await?)
    }
}
struct Connections {
    checked: Mutex<Vec<SubjectId>>,
    result: Result<(), SandboxCredentialPrerequisiteError>,
    revoke: Option<Arc<WorkerAuthority>>,
}
#[async_trait::async_trait]
impl SandboxCredentialPrerequisite for Connections {
    async fn check(&self, owner: &SubjectId) -> Result<(), SandboxCredentialPrerequisiteError> {
        self.checked.lock().unwrap().push(owner.clone());
        if let Some(authority) = &self.revoke {
            *authority.state.lock().unwrap() = Err(CommandAuthorizationError::Revoked);
        }
        self.result.clone()
    }
}
fn authority(actor: &str) -> Arc<WorkerAuthority> {
    Arc::new(WorkerAuthority {
        actor: actor.into(),
        calls: AtomicUsize::new(0),
        state: Mutex::new(Ok(())),
    })
}

#[tokio::test]
async fn sandbox_controls_worker_rechecks_actor_after_owner_connection_io() {
    for revoke in [false, true] {
        let f = Fixture::new().await;
        f.share(ShareMode::OrgRoles, None).await;
        let record = f
            .admit("recipient", SandboxControl::Revive, "worker", 1)
            .await
            .unwrap();
        let actor = authority("recipient");
        let connections = Arc::new(Connections {
            checked: Mutex::new(vec![]),
            result: Ok(()),
            revoke: revoke.then(|| actor.clone()),
        });
        let worker = SandboxControlWorker::new(
            f.store.clone(),
            f.access.clone(),
            actor.clone(),
            connections.clone(),
        );
        worker.run_one(&record.envelope.command_id).await.unwrap();
        assert_eq!(
            *connections.checked.lock().unwrap(),
            [SubjectId::new("owner")]
        );
        assert_eq!(actor.calls.load(Ordering::SeqCst), 2);
        let stored = f
            .store
            .sandbox_control(&record.envelope.command_id)
            .await
            .unwrap()
            .unwrap();
        if revoke {
            assert!(matches!(
                stored.status,
                SandboxControlStatus::Rejected {
                    reason: SandboxControlFailure::AuthorityRevoked,
                    ..
                }
            ));
            assert!(stored.execution_audit.is_none());
        } else {
            assert!(matches!(
                stored.status,
                SandboxControlStatus::Applied {
                    disposition: SandboxControlDisposition::AlreadySatisfied,
                    ..
                }
            ));
            assert_eq!(
                stored.execution_audit.unwrap().actor,
                context("recipient").actor
            );
        }
        worker.run_one(&record.envelope.command_id).await.unwrap();
        assert_eq!(actor.calls.load(Ordering::SeqCst), 2);
    }
}

#[tokio::test]
async fn sandbox_controls_worker_transient_authority_recovers_and_revocation_is_terminal() {
    for failure in [
        CommandAuthorizationError::Unavailable,
        CommandAuthorizationError::Revoked,
    ] {
        let f = Fixture::new().await;
        let record = f
            .admit("owner", SandboxControl::Pause, "custody", 1)
            .await
            .unwrap();
        let actor = authority("owner");
        *actor.state.lock().unwrap() = Err(failure.clone());
        let connections = Arc::new(Connections {
            checked: Mutex::new(vec![]),
            result: Ok(()),
            revoke: None,
        });
        let worker = SandboxControlWorker::new(
            f.store.clone(),
            f.access.clone(),
            actor.clone(),
            connections.clone(),
        );
        let first = worker.run_one(&record.envelope.command_id).await;
        assert_eq!(
            first.is_err(),
            failure == CommandAuthorizationError::Unavailable
        );
        *actor.state.lock().unwrap() = Ok(());
        worker.run_one(&record.envelope.command_id).await.unwrap();
        let stored = f
            .store
            .sandbox_control(&record.envelope.command_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            matches!(stored.status, SandboxControlStatus::Applied { .. }),
            failure == CommandAuthorizationError::Unavailable
        );
        assert!(connections.checked.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn sandbox_controls_worker_connection_failure_is_typed_and_preserves_target() {
    for (failure, expected) in [
        (
            SandboxCredentialPrerequisiteError::SetupRequired,
            Some(SandboxControlFailure::CredentialSetupRequired),
        ),
        (
            SandboxCredentialPrerequisiteError::ConnectionsRequired { services: vec![] },
            Some(SandboxControlFailure::ConnectionsRequired),
        ),
        (
            SandboxCredentialPrerequisiteError::Unavailable("private detail".into()),
            None,
        ),
    ] {
        let f = Fixture::new().await;
        let record = f
            .admit("owner", SandboxControl::Revive, "connections", 1)
            .await
            .unwrap();
        let actor = authority("owner");
        let connections = Arc::new(Connections {
            checked: Mutex::new(vec![]),
            result: Err(failure),
            revoke: None,
        });
        let before = f.store.lock().sandboxes.clone();
        let worker = SandboxControlWorker::new(
            f.store.clone(),
            f.access.clone(),
            actor.clone(),
            connections,
        );
        let result = worker.run_one(&record.envelope.command_id).await;
        let stored = f
            .store
            .sandbox_control(&record.envelope.command_id)
            .await
            .unwrap()
            .unwrap();
        if let Some(expected) = expected {
            result.unwrap();
            assert!(
                matches!(stored.status, SandboxControlStatus::Rejected { reason, .. } if reason == expected)
            );
        } else {
            assert_eq!(result, Err(SandboxControlError::Unavailable));
            assert_eq!(stored, record);
        }
        assert_eq!(actor.calls.load(Ordering::SeqCst), 2);
        assert_eq!(f.store.lock().sandboxes, before);
    }
}

#[tokio::test]
async fn sandbox_controls_worker_expires_abandoned_queue_before_authority_io() {
    let f = Fixture::new().await;
    let mut record = f
        .admit("owner", SandboxControl::Pause, "expire", 1)
        .await
        .unwrap();
    let now = SystemTime::now();
    record.admitted_at = crate::rfc3339_at(now - Duration::from_secs(301));
    record.queue_deadline = crate::rfc3339_at(now - Duration::from_secs(1));
    f.store
        .lock()
        .sandbox_controls
        .insert(record.envelope.command_id.clone(), record.clone());
    let actor = authority("owner");
    let connections = Arc::new(Connections {
        checked: Mutex::new(vec![]),
        result: Ok(()),
        revoke: None,
    });
    let worker = SandboxControlWorker::new(
        f.store.clone(),
        f.access.clone(),
        actor.clone(),
        connections,
    );
    worker.run_one(&record.envelope.command_id).await.unwrap();
    let stored = f
        .store
        .sandbox_control(&record.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        stored.status,
        SandboxControlStatus::Rejected {
            reason: SandboxControlFailure::QueueExpired,
            ..
        }
    ));
    assert_eq!(actor.calls.load(Ordering::SeqCst), 0);
}
