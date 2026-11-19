use std::{
    io,
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    task::{Context, Poll},
};

use futures_util::{FutureExt, StreamExt};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf};
use tokio_tungstenite::{
    tungstenite::{protocol::frame::FrameHeader, protocol::Role, Message},
    WebSocketStream,
};

use super::AdmissionIo;
use crate::ingress_budget::{
    AllocationPlan, ClassBounds, IngressBudget, IngressClass, IngressOwnershipPolicy,
};

#[path = "fragment_tests.rs"]
mod fragments;

#[path = "tests_029.rs"]
mod version_029;

#[path = "allocation_tests.rs"]
mod allocation;

struct Input {
    bytes: &'static [u8],
    consumed: Arc<AtomicUsize>,
    split: bool,
    pending: bool,
    generated_payload: usize,
}

impl Input {
    fn new(bytes: &'static [u8], split: bool) -> Self {
        Self {
            bytes,
            consumed: Arc::new(AtomicUsize::new(0)),
            split,
            pending: false,
            generated_payload: 0,
        }
    }

    fn maximal() -> Self {
        let mut input = Self::new(&[0x82, 127, 0, 0, 0, 0, 0, 64, 0, 0], false);
        input.generated_payload = 4 * 1024 * 1024;
        input
    }
}

impl AsyncRead for Input {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        output: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.split && self.pending {
            self.pending = false;
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        let count = if self.bytes.is_empty() {
            let count = self.generated_payload.min(output.remaining());
            output.initialize_unfilled_to(count).fill(b'x');
            output.advance(count);
            self.generated_payload -= count;
            count
        } else {
            let count = self.bytes.len().min(output.remaining()).min(if self.split {
                1
            } else {
                usize::MAX
            });
            output.put_slice(&self.bytes[..count]);
            self.bytes = &self.bytes[count..];
            count
        };
        self.consumed.fetch_add(count, Ordering::SeqCst);
        self.pending = true;
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for Input {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(Ok(bytes.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn coalesced_frames_are_admitted_separately_before_their_headers_escape() {
    let input = Input::new(&[0x82, 1, b'a', 0x82, 1, b'b'], false);
    let consumed = input.consumed.clone();
    let positions = Arc::new(Mutex::new(Vec::new()));
    let observed = positions.clone();
    let admission = move |_: &FrameHeader, length| {
        assert_eq!(length, 1);
        observed
            .lock()
            .unwrap()
            .push(consumed.load(Ordering::SeqCst));
        Ok(())
    };
    let mut socket = WebSocketStream::from_raw_socket(
        AdmissionIo::after_upgrade(input, admission),
        Role::Client,
        None,
    )
    .await;
    assert_eq!(
        socket.next().await.unwrap().unwrap(),
        Message::Binary(vec![b'a'])
    );
    assert_eq!(*positions.lock().unwrap(), [2]);
    assert_eq!(
        socket.next().await.unwrap().unwrap(),
        Message::Binary(vec![b'b'])
    );
    assert_eq!(*positions.lock().unwrap(), [2, 5]);
}

#[tokio::test]
async fn cancelled_partial_header_preserves_bytes_without_early_admission() {
    let input = Input::new(&[0x82, 126, 0, 1, b'x'], true);
    let attempts = Arc::new(AtomicUsize::new(0));
    let observed = attempts.clone();
    let mut io = AdmissionIo::after_upgrade(input, move |_: &FrameHeader, length| {
        assert_eq!(length, 1);
        observed.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    let mut output = [0; 32];
    assert!(io.read(&mut output).now_or_never().is_none());
    assert_eq!(attempts.load(Ordering::SeqCst), 0);
    assert_eq!(io.read(&mut output).await.unwrap(), 4);
    assert_eq!(&output[..4], &[0x82, 126, 0, 1]);
    assert_eq!(io.read(&mut output).await.unwrap(), 1);
    assert_eq!(output[0], b'x');
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn refusal_is_terminal_without_exposing_payload_or_reading_the_next_header() {
    let input = Input::new(&[0x82, 127, 0, 0, 0, 0, 0, 64, 0, 0, b'x'], true);
    let consumed = input.consumed.clone();
    let attempts = Arc::new(AtomicUsize::new(0));
    let observed = attempts.clone();
    let mut io = AdmissionIo::after_upgrade(input, move |_: &FrameHeader, length| {
        assert_eq!(length, 4 * 1024 * 1024);
        observed.fetch_add(1, Ordering::SeqCst);
        Err(io::ErrorKind::PermissionDenied.into())
    });
    let mut output = [0x55; 32];
    assert_eq!(
        io.read(&mut output).await.unwrap_err().kind(),
        io::ErrorKind::PermissionDenied
    );
    assert_eq!(output, [0x55; 32]);
    assert_eq!(consumed.load(Ordering::SeqCst), 10);
    assert_eq!(
        io.read(&mut output).await.unwrap_err().kind(),
        io::ErrorKind::BrokenPipe
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn truncated_headers_and_payloads_fail_but_clean_eof_is_stable() {
    for bytes in [&[0x82][..], &[0x82, 2, b'x'][..]] {
        let mut io =
            AdmissionIo::after_upgrade(Input::new(bytes, false), |_: &FrameHeader, _| Ok(()));
        let mut output = Vec::new();
        assert_eq!(
            io.read_to_end(&mut output).await.unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }
    let mut io = AdmissionIo::after_upgrade(Input::new(&[], false), |_: &FrameHeader, _| Ok(()));
    let mut output = [0];
    assert_eq!(io.read(&mut output).await.unwrap(), 0);
    assert_eq!(io.read(&mut output).await.unwrap(), 0);
}

#[tokio::test]
async fn zero_length_control_frame_does_not_swallow_the_following_data_header() {
    let admission = |_: &FrameHeader, _| Ok(());
    let input = Input::new(&[0x89, 0, 0x82, 1, b'x'], false);
    let mut socket = WebSocketStream::from_raw_socket(
        AdmissionIo::after_upgrade(input, admission),
        Role::Client,
        None,
    )
    .await;
    assert_eq!(socket.next().await.unwrap().unwrap(), Message::Ping(vec![]));
    assert_eq!(
        socket.next().await.unwrap().unwrap(),
        Message::Binary(vec![b'x'])
    );
}

#[tokio::test]
async fn saturated_data_class_refuses_before_codec_payload_allocation() {
    let root = IngressBudget::new(64 * 1024);
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(
            root.capacity_bytes(),
            AllocationPlan::new("control", &[16 * 1024]).unwrap(),
            AllocationPlan::new("transform", &[16 * 1024]).unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    let occupied = policy
        .reserve(
            IngressClass::Data,
            AllocationPlan::new("occupied", &[policy.bounds().data_bytes()]).unwrap(),
        )
        .unwrap();
    let initial = policy
        .reserve(
            IngressClass::Control,
            AllocationPlan::new("codec_initial", &[8192]).unwrap(),
        )
        .unwrap();
    let input = Input::maximal();
    let consumed = input.consumed.clone();
    let admission = |_: &FrameHeader, length| {
        let plan = AllocationPlan::new("incoming", &[length as usize]).unwrap();
        match policy.reserve(IngressClass::Data, plan) {
            Err(error) => Err(io::Error::other(error)),
            Ok(_) => panic!("saturated data class unexpectedly admitted a frame"),
        }
    };
    let mut probe = crate::allocation_probe::AllocationProbe::new();
    let operation = async {
        let mut socket = WebSocketStream::from_raw_socket(
            AdmissionIo::after_upgrade(input, admission),
            Role::Client,
            None,
        )
        .await;
        assert!(socket.next().await.unwrap().is_err());
        assert_eq!(consumed.load(Ordering::SeqCst), 10);
    };
    // The synthetic I/O is ready throughout, so the measured future completes
    // in one poll on this thread and includes the actual upstream codec.
    probe.track(|| operation.now_or_never().expect("ready synthetic I/O"));
    assert!(probe.peak() < 32 * 1024);
    assert_eq!(probe.live(), 0);
    drop(initial);
    drop(occupied);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[tokio::test]
async fn ungated_maximal_source_exceeds_the_pressure_test_allocation_bound() {
    let input = Input::maximal();
    let consumed = input.consumed.clone();
    let mut probe = crate::allocation_probe::AllocationProbe::new();
    let operation = async {
        let mut socket = WebSocketStream::from_raw_socket(input, Role::Client, None).await;
        let message = socket.next().await.unwrap().unwrap();
        assert_eq!(message.len(), 4 * 1024 * 1024);
        assert_eq!(consumed.load(Ordering::SeqCst), 4 * 1024 * 1024 + 10);
    };
    probe.track(|| operation.now_or_never().expect("ready synthetic I/O"));
    assert!(probe.peak() > 4 * 1024 * 1024);
    assert_eq!(probe.live(), 0);
}
