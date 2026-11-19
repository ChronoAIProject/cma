use super::*;
use layer3_adapter::owner_protocol::{
    OwnershipFailure, OwnershipPhase, ReceiverRevocation, TransportAuthority,
};

fn authority() -> Arc<Authority> {
    Authority::new(
        OwnershipToken {
            agent_id: crate::AgentId("agent".into()),
            instance_id: "manager".into(),
            epoch: 1,
            protocol: 1,
        },
        Instant::now() + std::time::Duration::from_secs(30),
    )
}

struct Receiver;
#[async_trait::async_trait]
impl ReceiverRevocation for Receiver {
    fn identity(&self) -> String {
        "receiver".into()
    }
    async fn revoke(&self) -> layer3_adapter::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn retry_evidence_is_independent_of_disabled_diagnostics_and_survives_cancellation() {
    for enabled in [false, true] {
        let authority = authority();
        let evidence = enabled.then(|| authority.observe_retry_evidence().unwrap());
        let cause = OwnershipFailure::http(OwnershipPhase::Challenge, 404);
        let result = authority
            .run(async {
                authority.record_failure_and_revoke(cause).await;
                std::future::pending::<()>().await;
            })
            .await;
        assert!(result.is_err());
        assert!(!authority.is_current());
        assert!(authority.receiver_failure().is_none());
        assert_eq!(authority.retry_evidence.get().is_some(), enabled);
        if let Some(evidence) = evidence {
            assert_eq!(evidence.first_failure().unwrap().failure(), cause);
            authority
                .record_failure(OwnershipFailure::http(OwnershipPhase::Revoke, 403))
                .await;
            assert_eq!(evidence.first_failure().unwrap().failure(), cause);
        }
    }
}

#[tokio::test]
async fn observer_cannot_be_installed_after_receiver_registration() {
    let authority = authority();
    authority.register_receiver(Arc::new(Receiver)).unwrap();
    assert!(authority.observe_retry_evidence().is_none());
    assert!(authority.retry_evidence.get().is_none());
}
