use super::*;
use crate::{
    agent_ownership::{OwnershipToken, PROTOCOL_VERSION},
    AgentId,
};
use std::sync::Mutex;

#[derive(Default)]
struct MissingReceiver {
    cleanup: Mutex<Vec<ReceiverConfirmationCustody>>,
}

#[async_trait::async_trait]
impl ReceiverRevocation for MissingReceiver {
    fn identity(&self) -> String {
        "same-runtime-instance".into()
    }

    async fn revoke(&self) -> layer3_adapter::Result<()> {
        Err(AdapterError::Unavailable("fixture has no ACK".into()))
    }

    async fn settle(
        &self,
        custody: ReceiverConfirmationCustody,
    ) -> layer3_adapter::Result<ReceiverCleanup> {
        self.cleanup.lock().unwrap().push(custody);
        if custody == ReceiverConfirmationCustody::NeverSent {
            Ok(ReceiverCleanup::NoReceiver)
        } else {
            self.revoke().await?;
            Ok(ReceiverCleanup::Acknowledged)
        }
    }
}

fn authority() -> Arc<Authority> {
    Authority::new(
        OwnershipToken {
            protocol: PROTOCOL_VERSION,
            agent_id: AgentId("fixture-agent".into()),
            instance_id: "fixture-manager".into(),
            epoch: 1,
        },
        tokio::time::Instant::now() + Duration::from_secs(5),
    )
}

#[tokio::test]
async fn re_registration_preserves_possible_confirmation_and_observed_receiver() {
    for expected in [
        ReceiverConfirmationCustody::PossiblyDelivered,
        ReceiverConfirmationCustody::ReceiverObserved,
    ] {
        let authority = authority();
        let original = Arc::new(MissingReceiver::default());
        let replacement = Arc::new(MissingReceiver::default());
        authority.register_receiver(original.clone()).unwrap();
        let identity = original.identity();
        if expected == ReceiverConfirmationCustody::PossiblyDelivered {
            authority.begin_receiver_confirmation(&identity).unwrap();
        }
        authority.receiver_observed(&identity);
        authority.register_receiver(replacement.clone()).unwrap();
        assert!(authority
            .revoke_transports_after_local_drain()
            .await
            .is_err());
        assert_eq!(*original.cleanup.lock().unwrap(), [expected]);
        assert!(replacement.cleanup.lock().unwrap().is_empty());
        assert!(authority.begin_receiver_confirmation(&identity).is_err());
    }
}

#[tokio::test]
async fn a_negative_receiver_proof_requires_the_local_drain_path() {
    let authority = authority();
    let receiver = Arc::new(MissingReceiver::default());
    let identity = receiver.identity();
    authority.register_receiver(receiver.clone()).unwrap();

    assert!(authority.revoke_transports().await.is_err());
    assert!(receiver.cleanup.lock().unwrap().is_empty());
    assert!(authority.begin_receiver_confirmation(&identity).is_err());

    authority
        .revoke_transports_after_local_drain()
        .await
        .unwrap();
    assert_eq!(
        *receiver.cleanup.lock().unwrap(),
        [ReceiverConfirmationCustody::NeverSent]
    );
}

#[tokio::test(start_paused = true)]
async fn a_join_timeout_never_settles_a_receiver() {
    let authority = authority();
    let receiver = Arc::new(MissingReceiver::default());
    authority.register_receiver(receiver.clone()).unwrap();
    let pending = authority.admit_work().unwrap();

    assert!(tokio::time::timeout(
        Duration::from_secs(1),
        authority.revoke_transports_after_local_drain(),
    )
    .await
    .is_err());
    assert!(receiver.cleanup.lock().unwrap().is_empty());
    assert!(authority.admit_work().is_err());

    drop(pending);
    authority
        .revoke_transports_after_local_drain()
        .await
        .unwrap();
    assert_eq!(
        *receiver.cleanup.lock().unwrap(),
        [ReceiverConfirmationCustody::NeverSent]
    );
}
