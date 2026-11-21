use super::*;
use crate::desktop_telemetry::Counters;
use futures_util::{sink, stream};
use std::{collections::VecDeque, future::pending};
use tokio::sync::mpsc;

#[derive(Default)]
struct Runtime {
    outgoing: VecDeque<Result<Option<DesktopFrame>, DesktopError>>,
    sent: Vec<DesktopFrame>,
    send_delay: Duration,
    send_error: Option<DesktopError>,
    check_refused: bool,
    checks: usize,
    invalidate_after: Option<Duration>,
}

impl RelayRuntime for Runtime {
    async fn still_authorized(&mut self) -> bool {
        self.checks += 1;
        !self.check_refused
    }

    async fn send(&mut self, frame: DesktopFrame) -> Result<(), DesktopError> {
        if !self.send_delay.is_zero() {
            tokio::time::sleep(self.send_delay).await;
        }
        if let Some(error) = self.send_error.take() {
            return Err(error);
        }
        self.sent.push(frame);
        Ok(())
    }

    async fn recv(&mut self) -> Result<Option<DesktopFrame>, DesktopError> {
        match self.outgoing.pop_front() {
            Some(result) => result,
            None => pending().await,
        }
    }

    async fn while_authorized<T: Send>(
        &mut self,
        operation: impl Future<Output = T> + Send,
    ) -> Result<T, DesktopError> {
        if let Some(delay) = self.invalidate_after {
            return tokio::select! {
                biased;
                _ = tokio::time::sleep(delay) => Err(DesktopError::Unavailable),
                result = operation => Ok(result),
            };
        }
        Ok(operation.await)
    }
}

#[derive(Clone, Copy)]
enum ClientSend {
    Ready,
    Fail,
    Blocked,
}

async fn exercise(
    runtime: &mut Runtime,
    incoming: impl Stream<Item = Result<Message, ()>>,
    client_send: ClientSend,
) -> (CloseReason, Vec<Message>) {
    let (output, mut received) = mpsc::unbounded_channel();
    let mut sink = Box::pin(sink::unfold(output, |output, message| async move {
        match client_send {
            ClientSend::Ready => output.send(message).unwrap(),
            ClientSend::Fail => return Err(()),
            ClientSend::Blocked => pending().await,
        }
        Ok(output)
    }));
    let mut source = Box::pin(incoming);
    let counters = Counters::default();
    let mut observation = RelayObservation::new(&counters);
    let reason = relay_frames(&mut sink, &mut source, runtime, &mut observation).await;
    observation.finish(reason);
    let mut sent = Vec::new();
    while let Ok(frame) = received.try_recv() {
        sent.push(frame);
    }
    (reason, sent)
}

fn incoming(
    frames: impl IntoIterator<Item = Result<Message, ()>>,
) -> impl Stream<Item = Result<Message, ()>> {
    stream::iter(frames).chain(stream::pending())
}

#[tokio::test(start_paused = true)]
async fn forwards_frames_in_both_directions_and_stops_at_the_client_close() {
    let mut runtime = Runtime {
        outgoing: [
            Ok(Some(DesktopFrame::Text("fixture".into()))),
            Ok(Some(DesktopFrame::Ping(vec![5]))),
        ]
        .into(),
        ..Default::default()
    };
    let source = stream::iter([
        Ok(Message::Binary(vec![1, 2].into())),
        Ok(Message::Pong(vec![3].into())),
    ])
    .chain(stream::once(async {
        tokio::time::sleep(Duration::from_secs(1)).await;
        Ok(Message::Close(None))
    }));
    let (reason, sent) = exercise(&mut runtime, source, ClientSend::Ready).await;
    assert_eq!(reason, CloseReason::ClientClosed);
    assert_eq!(
        sent,
        [
            Message::Text("fixture".into()),
            Message::Ping(vec![5].into())
        ]
    );
    assert_eq!(
        runtime.sent,
        [
            DesktopFrame::Binary(vec![1, 2]),
            DesktopFrame::Pong(vec![3])
        ]
    );
    assert_eq!(runtime.checks, 1);
}

#[tokio::test(start_paused = true)]
async fn classifies_read_and_write_closes_without_forwarding_after_failure() {
    for (outgoing, expected) in [
        (Ok(None), CloseReason::RuntimeClosed),
        (Ok(Some(DesktopFrame::Close)), CloseReason::RuntimeClosed),
        (
            Err(DesktopError::RuntimeUnavailable),
            CloseReason::RuntimeError,
        ),
        (
            Err(DesktopError::Unavailable),
            CloseReason::AuthorizationRefused,
        ),
    ] {
        let mut runtime = Runtime {
            outgoing: [outgoing].into(),
            ..Default::default()
        };
        assert_eq!(
            exercise(&mut runtime, incoming([]), ClientSend::Ready).await,
            (expected, vec![])
        );
    }

    let mut runtime = Runtime::default();
    assert_eq!(
        exercise(&mut runtime, incoming([Err(())]), ClientSend::Ready).await,
        (CloseReason::ClientError, vec![])
    );
    runtime
        .outgoing
        .push_back(Ok(Some(DesktopFrame::Binary(vec![1]))));
    assert_eq!(
        exercise(&mut runtime, incoming([]), ClientSend::Fail).await,
        (CloseReason::ClientError, vec![])
    );
    runtime.send_error = Some(DesktopError::RuntimeUnavailable);
    assert_eq!(
        exercise(
            &mut runtime,
            incoming([Ok(Message::Binary(vec![1].into()))]),
            ClientSend::Ready
        )
        .await,
        (CloseReason::RuntimeError, vec![])
    );
    assert!(runtime.sent.is_empty());
}

#[tokio::test(start_paused = true)]
async fn blocked_sends_keep_the_existing_deadline_in_both_directions() {
    let mut runtime = Runtime {
        send_delay: Duration::from_secs(15),
        ..Default::default()
    };
    let started = Instant::now();
    assert_eq!(
        exercise(
            &mut runtime,
            incoming([Ok(Message::Binary(vec![1].into()))]),
            ClientSend::Ready
        )
        .await,
        (CloseReason::RuntimeSendTimeout, vec![])
    );
    assert_eq!(started.elapsed(), Duration::from_secs(10));
    assert!(runtime.sent.is_empty());

    runtime
        .outgoing
        .push_back(Ok(Some(DesktopFrame::Binary(vec![2]))));
    let started = Instant::now();
    assert_eq!(
        exercise(&mut runtime, incoming([]), ClientSend::Blocked).await,
        (CloseReason::ClientSendTimeout, vec![])
    );
    assert_eq!(started.elapsed(), Duration::from_secs(10));
}

#[tokio::test(start_paused = true)]
async fn lifetime_and_authority_loss_keep_their_terminal_paths() {
    let mut runtime = Runtime::default();
    let started = Instant::now();
    assert_eq!(
        exercise(&mut runtime, incoming([]), ClientSend::Ready).await,
        (CloseReason::Lifetime, vec![])
    );
    assert_eq!(started.elapsed(), Duration::from_secs(30 * 60));
    assert!((60..=61).contains(&runtime.checks));

    runtime.check_refused = true;
    assert_eq!(
        exercise(&mut runtime, incoming([]), ClientSend::Ready).await,
        (CloseReason::AuthorizationRefused, vec![])
    );
    runtime.check_refused = false;
    runtime
        .outgoing
        .push_back(Ok(Some(DesktopFrame::Binary(vec![3]))));
    runtime.invalidate_after = Some(Duration::from_secs(2));
    let started = Instant::now();
    assert_eq!(
        exercise(&mut runtime, incoming([]), ClientSend::Blocked).await,
        (CloseReason::AuthorizationRefused, vec![])
    );
    assert_eq!(started.elapsed(), Duration::from_secs(2));
}
