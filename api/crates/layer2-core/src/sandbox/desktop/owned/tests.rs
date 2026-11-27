use super::*;
use tokio::sync::oneshot;

struct Control;
impl DesktopOwnershipControl for Control {}

fn state() -> (watch::Sender<Option<Instant>>, ConnectionState) {
    let (validity, receiver) = watch::channel(Some(Instant::now() + Duration::from_secs(1)));
    let connection = ConnectionState {
        authority: Arc::new(Mutex::new(DesktopOwnership {
            sandbox_id: "sandbox-fixture".into(),
            session_id: "session-fixture".into(),
            runtime_id: "runtime-fixture".into(),
            boot_id: None,
            epoch: 1,
            issued_at_ms: 1,
            expires_at_ms: 15_001,
        })),
        control: Arc::new(Control),
        validity: receiver,
        renewal: None,
    };
    (validity, connection)
}

#[tokio::test(start_paused = true)]
async fn current_authority_allows_publication_but_revocation_and_loss_refuse_it() {
    let (validity, mut state) = state();
    assert_eq!(state.while_authorized(async { 42 }).await.unwrap(), 42);
    validity.send_replace(None);
    assert!(state.while_authorized(async { 42 }).await.is_err());
    drop(validity);
    assert!(!state.valid());
}

#[tokio::test(start_paused = true)]
async fn expiry_wins_when_blocked_publication_becomes_ready_after_its_deadline() {
    let (_validity, mut state) = state();
    let (ready, waiting) = oneshot::channel();
    let mut publication = Box::pin(state.while_authorized(waiting));
    assert!(futures_util::poll!(&mut publication).is_pending());
    tokio::time::advance(Duration::from_secs(2)).await;
    ready.send(()).unwrap();
    assert!(publication.await.is_err());
}

#[tokio::test(start_paused = true)]
async fn pending_publication_uses_the_deadline_of_a_confirmed_renewal() {
    let (validity, mut state) = state();
    let (ready, waiting) = oneshot::channel();
    let mut publication = Box::pin(state.while_authorized(waiting));
    assert!(futures_util::poll!(&mut publication).is_pending());
    validity.send_replace(Some(Instant::now() + Duration::from_secs(5)));
    tokio::time::advance(Duration::from_secs(2)).await;
    ready.send(42).unwrap();
    assert_eq!(publication.await.unwrap().unwrap(), 42);
}

#[tokio::test(start_paused = true)]
async fn pending_publication_stops_on_revocation_after_a_renewal() {
    let (validity, mut state) = state();
    let (ready, waiting) = oneshot::channel();
    let mut publication = Box::pin(state.while_authorized(waiting));
    assert!(futures_util::poll!(&mut publication).is_pending());
    validity.send_replace(Some(Instant::now() + Duration::from_secs(5)));
    tokio::time::advance(Duration::from_secs(2)).await;
    assert!(futures_util::poll!(&mut publication).is_pending());
    validity.send_replace(None);
    ready.send(()).unwrap();
    assert!(publication.await.is_err());
}

struct Frames(oneshot::Receiver<()>);
#[async_trait::async_trait]
impl layer3_adapter::DesktopConnection for Frames {
    async fn recv(&mut self) -> layer3_adapter::Result<Option<DesktopFrame>> {
        (&mut self.0).await.unwrap();
        Ok(Some(DesktopFrame::Binary(vec![1])))
    }

    async fn send(&mut self, _: DesktopFrame) -> layer3_adapter::Result<()> {
        Ok(())
    }
}

#[tokio::test(start_paused = true)]
async fn expiry_wins_when_a_runtime_frame_arrives_after_its_deadline() {
    let (_validity, mut state) = state();
    let (ready, waiting) = oneshot::channel();
    let mut frames: DesktopConnectionHandle = Box::new(Frames(waiting));
    let mut receive = Box::pin(state.recv(&mut frames));
    assert!(futures_util::poll!(&mut receive).is_pending());
    tokio::time::advance(Duration::from_secs(2)).await;
    ready.send(()).unwrap();
    assert!(receive.await.is_err());
}

#[tokio::test(start_paused = true)]
async fn pending_runtime_read_survives_the_previous_deadline_after_renewal() {
    let (validity, mut state) = state();
    let (ready, waiting) = oneshot::channel();
    let mut frames: DesktopConnectionHandle = Box::new(Frames(waiting));
    let mut receive = Box::pin(state.recv(&mut frames));
    assert!(futures_util::poll!(&mut receive).is_pending());
    validity.send_replace(Some(Instant::now() + Duration::from_secs(5)));
    tokio::time::advance(Duration::from_secs(2)).await;
    ready.send(()).unwrap();
    assert!(matches!(
        receive.await.unwrap(),
        Some(DesktopFrame::Binary(bytes)) if bytes == [1]
    ));
}

#[tokio::test(start_paused = true)]
async fn late_renewal_ack_cannot_restore_expired_authority() {
    let current = Instant::now() + Duration::from_secs(1);
    let (acknowledge, waiting) = oneshot::channel();
    let mut renewal = Box::pin(renew_before_expiry(current, async {
        waiting.await.unwrap();
        Ok(Instant::now() + Duration::from_secs(10))
    }));
    assert!(futures_util::poll!(&mut renewal).is_pending());
    tokio::time::advance(Duration::from_secs(2)).await;
    acknowledge.send(()).unwrap();
    assert!(renewal.await.is_err());
}

#[tokio::test(start_paused = true)]
async fn expired_renewal_does_not_start_receiver_work() {
    let current = Instant::now();
    assert!(renew_before_expiry::<()>(current, async {
        panic!("expired authority must not reach the receiver")
    })
    .await
    .is_err());
}

#[tokio::test(start_paused = true)]
async fn timely_renewal_preserves_receiver_result() {
    let current = Instant::now() + Duration::from_secs(1);
    assert_eq!(
        renew_before_expiry(current, async { Ok(42) })
            .await
            .unwrap(),
        42
    );
    assert!(matches!(
        renew_before_expiry::<()>(current, async { Err(DesktopError::RuntimeUnavailable) }).await,
        Err(DesktopError::RuntimeUnavailable)
    ));
}
