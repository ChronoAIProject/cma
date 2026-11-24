use super::containment_test_support::{manager, Receiver, Store};
use super::*;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Clone, Default)]
struct Output(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for Output {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Output {
    type Writer = Self;
    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}

#[tokio::test(start_paused = true)]
async fn typed_cleanup_failure_keeps_its_class_and_hashed_receiver_lineage() {
    use layer3_adapter::owner_protocol::{
        ReceiverCleanup, ReceiverConfirmationCustody, ReceiverRevocation,
        ReceiverSettlementFailure as Failure, ReceiverSettlementWatch, TransportAuthority,
    };
    use sha2::{Digest, Sha256};

    struct ClassifiedReceiver(Failure);
    #[async_trait::async_trait]
    impl ReceiverRevocation for ClassifiedReceiver {
        fn identity(&self) -> String {
            "raw-receiver-sentinel".into()
        }
        fn sandbox_id(&self) -> Option<&str> {
            Some("raw-sandbox-sentinel")
        }
        async fn revoke(&self) -> layer3_adapter::Result<()> {
            panic!("containment must use typed settlement")
        }
        async fn settle_typed(
            &self,
            _: ReceiverConfirmationCustody,
            _: &ReceiverSettlementWatch,
        ) -> Result<ReceiverCleanup, Failure> {
            Err(self.0)
        }
    }

    for (failure, reason) in [
        (Failure::Unavailable, "transport_unavailable"),
        (Failure::Invalid, "authority_lost"),
    ] {
        let store = Arc::new(Store::default());
        let manager = manager(store.clone(), false, 2).await;
        let owner = manager
            .acquire(&AgentId("raw-agent-sentinel".into()))
            .await
            .unwrap()
            .unwrap();
        owner
            .register_receiver(Arc::new(ClassifiedReceiver(failure)))
            .unwrap();
        let output = Output::default();
        let subscriber = tracing_subscriber::fmt()
            .json()
            .without_time()
            .with_writer(output.clone())
            .finish();
        let _subscriber = tracing::subscriber::set_default(subscriber);
        let parent = tracing::info_span!("caller", identity = "raw-parent-sentinel");
        let _entered = parent.enter();
        let error = owner
            .revoke_transports_after_local_drain()
            .await
            .unwrap_err();
        assert_eq!(
            matches!(error, OwnershipError::Invalid(_)),
            failure == Failure::Invalid
        );
        assert_eq!(owner.receivers.lock().unwrap().len(), 1);
        assert_eq!(
            store
                .release_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        let captured = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
        let event: serde_json::Value = serde_json::from_str(captured.trim()).unwrap();
        let fields = &event["fields"];
        assert_eq!(fields["reason"], reason);
        assert_eq!(fields["stage"], "receiver_revocation");
        assert_eq!(fields["after_local_drain"], true);
        for (key, raw) in [
            ("agent_sha256", owner.token.agent_id.as_str()),
            ("instance_sha256", owner.token.instance_id.as_str()),
            ("receiver_sha256", "raw-receiver-sentinel"),
            ("sandbox_sha256", "raw-sandbox-sentinel"),
        ] {
            assert_eq!(fields[key], format!("{:x}", Sha256::digest(raw.as_bytes())));
            assert!(!captured.contains(raw), "{captured}");
        }
        assert!(!captured.contains("raw-parent-sentinel"), "{captured}");
    }
}

#[tokio::test(start_paused = true)]
async fn containment_events_distinguish_proofs_and_exclude_raw_identity_error_and_parent_fields() {
    let store = Arc::new(Store::default());
    let manager = manager(store.clone(), false, 2).await;
    let old = manager
        .acquire(&AgentId("raw-agent-sentinel".into()))
        .await
        .unwrap()
        .unwrap();
    Receiver::install(&old, "private-receiver-sentinel", true);
    let releasable = manager
        .acquire(&AgentId("raw-second-sentinel".into()))
        .await
        .unwrap()
        .unwrap();
    Receiver::install(&releasable, "another-receiver-sentinel", false);
    let output = Output::default();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_max_level(tracing::Level::DEBUG)
        .with_writer(output.clone())
        .finish();
    let _subscriber = tracing::subscriber::set_default(subscriber);
    let parent = tracing::info_span!("untrusted_parent", actor = "private-parent-sentinel");
    let _parent = parent.enter();
    assert_eq!(manager.retirement_background(&old).await.unwrap(), None);
    store.inner.release(&old.token).await.unwrap();
    let next = store
        .inner
        .acquire(&old.token.agent_id, &manager.instance_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        manager.retirement_background(&old).await.unwrap(),
        Some(Settlement::Superseded {
            newer_epoch: next.token.epoch
        })
    );
    assert_eq!(
        manager.retirement_background(&releasable).await.unwrap(),
        Some(Settlement::Released)
    );
    store.inner.partition(true);
    assert!(matches!(
        manager.maintain_step().await.unwrap(),
        MaintenanceStep::RetryAt(_)
    ));
    store.inner.partition(false);
    tokio::time::advance(Duration::from_millis(250)).await;
    assert!(matches!(
        manager.maintain_step().await.unwrap(),
        MaintenanceStep::Confirmed { .. }
    ));
    let captured = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    for outcome in [
        "pending",
        "superseded",
        "released",
        "retry_started",
        "recovered",
    ] {
        assert!(
            captured.contains(&format!("outcome=\"{outcome}\"")),
            "{captured}"
        );
    }
    for private in [
        "raw-agent-sentinel",
        "raw-second-sentinel",
        "private-receiver-sentinel",
        "another-receiver-sentinel",
        "private-parent-sentinel",
        "synthetic receiver unavailable",
        "synthetic store partition",
        "instance_id",
        "agent_id",
    ] {
        assert!(!captured.contains(private), "{captured}");
    }
    let superseded = captured
        .lines()
        .find(|line| line.contains("outcome=\"superseded\""))
        .unwrap();
    assert!(superseded.contains("receiver_housekeeping"));
    assert!(!superseded.contains("released") && !superseded.contains("ACK"));
}
