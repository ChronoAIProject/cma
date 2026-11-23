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
