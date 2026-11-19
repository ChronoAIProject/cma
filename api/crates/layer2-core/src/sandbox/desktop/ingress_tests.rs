use super::*;
use crate::sandbox::ingress_ownership::{
    sizing, AllocationPlan, ClassBounds, IngressBudget, IngressClass, IngressOwnershipPolicy,
};
use futures_util::{poll, Sink, SinkExt};
use std::{
    pin::Pin,
    sync::Mutex as StdMutex,
    task::{Context, Poll},
};
use tokio::sync::{mpsc, oneshot};

const PAYLOAD_BYTES: usize = 64 * 1024;

fn policy() -> IngressOwnershipPolicy {
    let data = PAYLOAD_BYTES + sizing::shared_owner_bytes::<DesktopFrame>();
    let floor = AllocationPlan::new("fixture progress", &[1024]).unwrap();
    let bounds = ClassBounds::new(data + 2048, floor, floor).unwrap();
    IngressOwnershipPolicy::new(IngressBudget::new(data + 2048), bounds).unwrap()
}

fn frame(policy: &IngressOwnershipPolicy) -> AdmittedFrame<DesktopFrame> {
    policy
        .reserve(
            IngressClass::Data,
            AllocationPlan::new(
                "fixture desktop frame",
                &[PAYLOAD_BYTES, sizing::shared_owner_bytes::<DesktopFrame>()],
            )
            .unwrap(),
        )
        .unwrap()
        .initialize(|| DesktopFrame::Binary(vec![0xa5; PAYLOAD_BYTES]))
}

struct Control;
impl DesktopOwnershipControl for Control {}

fn authority() -> (watch::Sender<Option<Instant>>, ConnectionState) {
    let (validity, receiver) = watch::channel(Some(Instant::now() + Duration::from_secs(1)));
    (
        validity,
        ConnectionState {
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
        },
    )
}

fn connection(
    transport: DesktopConnectionHandle,
    owned: Option<ConnectionState>,
) -> DesktopConnection {
    DesktopConnection {
        connection: Some(transport),
        sandbox_id: SandboxId("sandbox-fixture".into()),
        session_id: "session-fixture".into(),
        runtime_id: "runtime-fixture".into(),
        _lease: None,
        owned,
    }
}

type Queued = Arc<StdMutex<Option<AdmittedFrame<DesktopFrame>>>>;

struct Transport {
    input: mpsc::Receiver<AdmittedFrame<DesktopFrame>>,
    queued: Queued,
    flushed: oneshot::Receiver<layer3_adapter::Result<()>>,
}

#[async_trait::async_trait]
impl layer3_adapter::DesktopConnection for Transport {
    async fn recv(&mut self) -> layer3_adapter::Result<Option<DesktopFrame>> {
        panic!("admitted receive must not use the raw port")
    }

    async fn send(&mut self, _: DesktopFrame) -> layer3_adapter::Result<()> {
        panic!("admitted send must not use the raw port")
    }

    async fn recv_admitted(
        &mut self,
    ) -> layer3_adapter::Result<Option<AdmittedFrame<DesktopFrame>>> {
        Ok(self.input.recv().await)
    }

    async fn send_admitted(
        &mut self,
        frame: AdmittedFrame<DesktopFrame>,
    ) -> layer3_adapter::Result<()> {
        *self.queued.lock().unwrap() = Some(frame);
        (&mut self.flushed).await.unwrap()?;
        self.queued.lock().unwrap().take();
        Ok(())
    }
}

struct Fixture {
    connection: DesktopConnection,
    input: mpsc::Sender<AdmittedFrame<DesktopFrame>>,
    queued: Queued,
    flush: oneshot::Sender<layer3_adapter::Result<()>>,
}

fn fixture(owned: Option<ConnectionState>) -> Fixture {
    let (input, received) = mpsc::channel(1);
    let (flush, flushed) = oneshot::channel();
    let queued = Queued::default();
    Fixture {
        connection: connection(
            Box::new(Transport {
                input: received,
                queued: queued.clone(),
                flushed,
            }),
            owned,
        ),
        input,
        queued,
        flush,
    }
}

#[tokio::test]
async fn received_frame_keeps_the_same_owner_after_connection_teardown() {
    let policy = policy();
    let mut fixture = fixture(None);
    let original = frame(&policy);
    fixture.input.send(original.clone()).await.unwrap();
    let received = fixture.connection.recv_admitted().await.unwrap().unwrap();
    assert!(received.ptr_eq(&original));
    assert!(matches!(&*received, DesktopFrame::Binary(data) if data.len() == PAYLOAD_BYTES));
    drop(original);
    drop(fixture);
    assert_eq!(policy.budget(IngressClass::Data).available_bytes(), 0);
    drop(received);
    assert_eq!(
        policy.budget(IngressClass::Data).available_bytes(),
        policy.bounds().data_bytes()
    );
}

#[tokio::test]
async fn stalled_runtime_send_retains_custody_until_flush() {
    let policy = policy();
    let mut fixture = fixture(None);
    let original = frame(&policy);
    let mut send = Box::pin(fixture.connection.send_admitted(original.clone()));
    assert!(poll!(&mut send).is_pending());
    assert!(fixture
        .queued
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .ptr_eq(&original));
    drop(original);
    assert_eq!(policy.budget(IngressClass::Data).available_bytes(), 0);
    fixture.flush.send(Ok(())).unwrap();
    send.await.unwrap();
    assert!(fixture.queued.lock().unwrap().is_none());
    assert_eq!(
        policy.budget(IngressClass::Data).available_bytes(),
        policy.bounds().data_bytes()
    );
}

#[tokio::test]
async fn cancelled_and_failed_runtime_sends_leave_custody_with_the_queue() {
    for fail in [false, true] {
        let policy = policy();
        let mut fixture = fixture(None);
        let mut send = Box::pin(fixture.connection.send_admitted(frame(&policy)));
        assert!(poll!(&mut send).is_pending());
        if fail {
            fixture
                .flush
                .send(Err(layer3_adapter::AdapterError::Unavailable(
                    "fixture".into(),
                )))
                .unwrap();
            assert!(matches!(send.await, Err(DesktopError::RuntimeUnavailable)));
        } else {
            drop(send);
        }
        assert_eq!(policy.budget(IngressClass::Data).available_bytes(), 0);
        assert!(fixture.queued.lock().unwrap().is_some());
        // A queue can outlive both the send future and its transport handle.
        drop(fixture.connection);
        assert_eq!(policy.budget(IngressClass::Data).available_bytes(), 0);
        drop(fixture.queued);
        assert_eq!(
            policy.budget(IngressClass::Data).available_bytes(),
            policy.bounds().data_bytes()
        );
    }
}

#[tokio::test(start_paused = true)]
async fn receive_observes_a_renewed_deadline_without_losing_frame_custody() {
    let policy = policy();
    let (validity, owned) = authority();
    let mut fixture = fixture(Some(owned));
    let mut receive = Box::pin(fixture.connection.recv_admitted());
    assert!(poll!(&mut receive).is_pending());
    tokio::time::advance(Duration::from_millis(500)).await;
    validity.send_replace(Some(Instant::now() + Duration::from_secs(5)));
    assert!(poll!(&mut receive).is_pending());
    tokio::time::advance(Duration::from_secs(1)).await;
    fixture.input.send(frame(&policy)).await.unwrap();
    let received = receive.await.unwrap().unwrap();
    assert_eq!(policy.budget(IngressClass::Data).available_bytes(), 0);
    drop(received);
    assert_eq!(
        policy.budget(IngressClass::Data).available_bytes(),
        policy.bounds().data_bytes()
    );
}

#[tokio::test(start_paused = true)]
async fn expiry_revocation_and_control_loss_win_over_ready_admitted_frames() {
    for cause in 0..3 {
        let policy = policy();
        let (validity, owned) = authority();
        let mut fixture = fixture(Some(owned));
        let mut receive = Box::pin(fixture.connection.recv_admitted());
        assert!(poll!(&mut receive).is_pending());
        fixture.input.send(frame(&policy)).await.unwrap();
        match cause {
            0 => tokio::time::advance(Duration::from_secs(2)).await,
            1 => {
                validity.send_replace(None);
            }
            _ => drop(validity),
        }
        assert!(matches!(receive.await, Err(DesktopError::Unavailable)));
        assert_eq!(policy.budget(IngressClass::Data).available_bytes(), 0);
        drop(fixture);
        assert_eq!(
            policy.budget(IngressClass::Data).available_bytes(),
            policy.bounds().data_bytes()
        );
    }
}

#[tokio::test(start_paused = true)]
async fn revoked_send_keeps_queued_storage_charged_and_never_sends_again() {
    let policy = policy();
    let (validity, owned) = authority();
    let mut fixture = fixture(Some(owned));
    let mut send = Box::pin(fixture.connection.send_admitted(frame(&policy)));
    assert!(poll!(&mut send).is_pending());
    // Control admission remains available while the data class is full.
    let control = policy
        .reserve(
            IngressClass::Control,
            AllocationPlan::new("fixture control", &[1024]).unwrap(),
        )
        .unwrap()
        .initialize(|| vec![0_u8; 512]);
    validity.send_replace(None);
    assert!(matches!(send.await, Err(DesktopError::Unavailable)));
    assert_eq!(policy.budget(IngressClass::Data).available_bytes(), 0);
    let queued = fixture.queued.lock().unwrap().take().unwrap();
    assert!(matches!(
        fixture.connection.send_admitted(queued).await,
        Err(DesktopError::Unavailable)
    ));
    assert!(fixture.queued.lock().unwrap().is_none());
    assert_eq!(
        policy.budget(IngressClass::Data).available_bytes(),
        policy.bounds().data_bytes()
    );
    drop(control);
}

#[tokio::test(start_paused = true)]
async fn expiry_wins_over_a_late_ready_runtime_flush() {
    let policy = policy();
    let (_validity, owned) = authority();
    let mut fixture = fixture(Some(owned));
    let mut send = Box::pin(fixture.connection.send_admitted(frame(&policy)));
    assert!(poll!(&mut send).is_pending());
    tokio::time::advance(Duration::from_secs(2)).await;
    fixture.flush.send(Ok(())).unwrap();
    assert!(matches!(send.await, Err(DesktopError::Unavailable)));
    assert!(fixture.queued.lock().unwrap().is_some());
    assert_eq!(policy.budget(IngressClass::Data).available_bytes(), 0);
    drop(fixture.connection);
    drop(fixture.queued);
    assert_eq!(
        policy.budget(IngressClass::Data).available_bytes(),
        policy.bounds().data_bytes()
    );
}

#[tokio::test]
async fn cancelling_receive_preserves_the_producers_frame_for_the_next_attempt() {
    let policy = policy();
    let mut fixture = fixture(None);
    let mut receive = Box::pin(fixture.connection.recv_admitted());
    assert!(poll!(&mut receive).is_pending());
    fixture.input.send(frame(&policy)).await.unwrap();
    drop(receive);
    assert_eq!(policy.budget(IngressClass::Data).available_bytes(), 0);
    let received = fixture.connection.recv_admitted().await.unwrap().unwrap();
    assert!(matches!(&*received, DesktopFrame::Binary(data) if data.len() == PAYLOAD_BYTES));
    drop(received);
    assert_eq!(
        policy.budget(IngressClass::Data).available_bytes(),
        policy.bounds().data_bytes()
    );
}

#[derive(Default)]
struct FinalSink(Option<AdmittedFrame<DesktopFrame>>);

impl Sink<AdmittedFrame<DesktopFrame>> for FinalSink {
    type Error = std::io::Error;
    fn poll_ready(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
    fn start_send(
        mut self: Pin<&mut Self>,
        frame: AdmittedFrame<DesktopFrame>,
    ) -> Result<(), Self::Error> {
        self.0 = Some(frame);
        Ok(())
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Pending
    }
    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Pending
    }
}

#[tokio::test(start_paused = true)]
async fn final_sink_keeps_received_storage_after_authority_cancels_flush() {
    let policy = policy();
    let (validity, owned) = authority();
    let mut fixture = fixture(Some(owned));
    fixture.input.send(frame(&policy)).await.unwrap();
    let received = fixture.connection.recv_admitted().await.unwrap().unwrap();
    let mut sink = FinalSink::default();
    let mut send = Box::pin(fixture.connection.while_authorized(sink.send(received)));
    assert!(poll!(&mut send).is_pending());
    validity.send_replace(None);
    assert!(matches!(send.await, Err(DesktopError::Unavailable)));
    drop(fixture);
    assert!(sink.0.is_some());
    assert_eq!(policy.budget(IngressClass::Data).available_bytes(), 0);
    drop(sink);
    assert_eq!(
        policy.budget(IngressClass::Data).available_bytes(),
        policy.bounds().data_bytes()
    );
}

struct Legacy;

#[async_trait::async_trait]
impl layer3_adapter::DesktopConnection for Legacy {
    async fn recv(&mut self) -> layer3_adapter::Result<Option<DesktopFrame>> {
        Ok(Some(DesktopFrame::Close))
    }
    async fn send(&mut self, frame: DesktopFrame) -> layer3_adapter::Result<()> {
        assert_eq!(frame, DesktopFrame::Close);
        Ok(())
    }
}

#[tokio::test]
async fn legacy_ports_still_work_but_admitted_ports_explicitly_refuse() {
    use layer3_adapter::DesktopConnection as _;
    let policy = policy();
    assert!(matches!(
        Legacy.recv_admitted().await,
        Err(layer3_adapter::AdapterError::Unsupported(_))
    ));
    assert!(matches!(
        Legacy.send_admitted(frame(&policy)).await,
        Err(layer3_adapter::AdapterError::Unsupported(_))
    ));
    let mut connection = connection(Box::new(Legacy), None);
    assert_eq!(connection.recv().await.unwrap(), Some(DesktopFrame::Close));
    connection.send(DesktopFrame::Close).await.unwrap();
    assert!(matches!(
        connection.recv_admitted().await,
        Err(DesktopError::RuntimeUnavailable)
    ));
    assert!(matches!(
        connection.send_admitted(frame(&policy)).await,
        Err(DesktopError::RuntimeUnavailable)
    ));
    assert_eq!(
        policy.budget(IngressClass::Data).available_bytes(),
        policy.bounds().data_bytes()
    );
    connection.connection.take();
    assert!(matches!(
        connection.recv_admitted().await,
        Err(DesktopError::Unavailable)
    ));
    assert!(matches!(
        connection.send_admitted(frame(&policy)).await,
        Err(DesktopError::Unavailable)
    ));
    assert_eq!(
        policy.budget(IngressClass::Data).available_bytes(),
        policy.bounds().data_bytes()
    );
}

#[tokio::test]
async fn end_of_stream_remains_distinct_from_refusal() {
    let mut fixture = fixture(None);
    drop(fixture.input);
    assert!(fixture.connection.recv_admitted().await.unwrap().is_none());
}
