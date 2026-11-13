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
