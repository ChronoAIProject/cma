use super::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex,
};
use tokio::sync::mpsc;

struct Port {
    revision: Mutex<Option<String>>,
    connects: AtomicUsize,
    attempts: mpsc::UnboundedSender<usize>,
    first: Mutex<Option<EndReason>>,
    stopped: AtomicUsize,
}
impl SourcePort for Port {
    fn revision<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<String, GatewayError>> {
        Box::pin(async {
            self.revision
                .lock()
                .unwrap()
                .clone()
                .ok_or(GatewayError::Authority)
        })
    }
    fn connect<'a>(
        &'a self,
        _: &'a str,
        mut stop: watch::Receiver<bool>,
    ) -> BoxFuture<'a, Attempt> {
        Box::pin(async move {
            let n = self.connects.fetch_add(1, Ordering::SeqCst) + 1;
            let _ = self.attempts.send(n);
            let immediate = self.first.lock().unwrap().take();
            let reason = if let Some(reason) = immediate {
                reason
            } else {
                let _ = stop.changed().await;
                self.stopped.fetch_add(1, Ordering::SeqCst);
                EndReason::Stopped
            };
            Attempt {
                result: Ok(GatewayEnd {
                    reason,
                    checkpoint: None,
                    retry_after: Duration::from_millis(1),
                }),
                cooldown: Duration::ZERO,
            }
        })
    }
}
fn timing() -> Timing {
    Timing {
        poll: Duration::from_millis(5),
        retry: Duration::from_millis(1),
        stop: Duration::from_millis(100),
    }
}
fn fixture(first: Option<EndReason>) -> (Arc<Port>, mpsc::UnboundedReceiver<usize>) {
    let (attempts, rx) = mpsc::unbounded_channel();
    (
        Arc::new(Port {
            revision: Mutex::new(Some("initial".into())),
            connects: AtomicUsize::new(0),
            attempts,
            first: Mutex::new(first),
            stopped: AtomicUsize::new(0),
        }),
        rx,
    )
}
async fn next(rx: &mut mpsc::UnboundedReceiver<usize>) -> usize {
    tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .unwrap()
        .unwrap()
}
#[tokio::test]
async fn reconnect_reacquires_and_shutdown_stops_the_owned_connection() {
    let (port, mut rx) = fixture(Some(EndReason::Reconnect));
    let (stop, stopped) = watch::channel(false);
    let task = tokio::spawn(run(port.clone(), "record".into(), stopped, timing()));
    assert_eq!(next(&mut rx).await, 1);
    assert_eq!(next(&mut rx).await, 2);
    stop.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(port.stopped.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn source_revision_change_stops_old_socket_and_starts_current_revision() {
    let (port, mut rx) = fixture(None);
    let (stop, stopped) = watch::channel(false);
    let task = tokio::spawn(run(port.clone(), "record".into(), stopped, timing()));
    assert_eq!(next(&mut rx).await, 1);
    *port.revision.lock().unwrap() = Some("rotated".into());
    assert_eq!(next(&mut rx).await, 2);
    assert_eq!(port.stopped.load(Ordering::SeqCst), 1);
    *port.revision.lock().unwrap() = None;
    tokio::time::sleep(Duration::from_millis(25)).await;
    assert_eq!(port.stopped.load(Ordering::SeqCst), 2);
    assert_eq!(port.connects.load(Ordering::SeqCst), 2);
    stop.send(true).unwrap();
    task.await.unwrap();
}
#[tokio::test]
async fn terminal_authentication_error_requires_a_new_source_revision() {
    let (port, mut rx) = fixture(Some(EndReason::AuthenticationFailed));
    let (stop, stopped) = watch::channel(false);
    let task = tokio::spawn(run(port.clone(), "record".into(), stopped, timing()));
    assert_eq!(next(&mut rx).await, 1);
    tokio::time::sleep(Duration::from_millis(25)).await;
    assert_eq!(port.connects.load(Ordering::SeqCst), 1);
    *port.revision.lock().unwrap() = Some("fixed-credential".into());
    assert_eq!(next(&mut rx).await, 2);
    stop.send(true).unwrap();
    task.await.unwrap();
}
#[test]
fn retry_delay_never_shortens_gateway_or_discovery_cooldown() {
    let attempt = Attempt {
        result: Ok(GatewayEnd {
            reason: EndReason::RateLimited,
            checkpoint: None,
            retry_after: Duration::from_secs(60),
        }),
        cooldown: Duration::from_secs(120),
    };
    assert_eq!(
        retry_delay(&attempt, 1, Duration::from_secs(5)),
        Duration::from_secs(120)
    );
    let busy = Attempt {
        result: Err(GatewayError::Authority),
        cooldown: Duration::ZERO,
    };
    assert_eq!(
        retry_delay(&busy, 1, Duration::from_secs(5)),
        Duration::from_secs(30)
    );
}
