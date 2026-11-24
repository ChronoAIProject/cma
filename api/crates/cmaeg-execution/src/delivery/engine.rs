//! Dispatch delivery commands to bounded workers and replace failed workers.
use super::queue;
use std::{future::Future, sync::Arc};
use tokio::{
    sync::{mpsc::error::SendError, oneshot},
    task::JoinSet,
};

pub const DELIVERY_WORKER_RESTARTS: &str = "cmaeg_delivery_worker_restarts_total";
const WORKER_QUEUE: usize = 64;

/// Routes each command to the worker `route` names, below `workers`. A worker
/// that stopped, for example by panicking, is replaced so one fault never halts
/// delivery for every channel; only `stop` or a closed inbound queue ends it.
pub async fn dispatch<C, S, F>(
    receiver: impl Into<queue::Input<C>>,
    mut stop: oneshot::Receiver<()>,
    workers: usize,
    route: impl Fn(&C) -> usize,
    spawn: S,
) where
    C: Send + 'static,
    S: Fn(queue::Receiver<C>) -> F,
    F: Future<Output = ()> + Send + 'static,
{
    let mut receiver = receiver.into();
    let inventory = Arc::new(queue::Inventory::default());
    let _monitor = inventory.monitor();
    metrics::gauge!("cmaeg_delivery_queue_capacity", "stage" => "worker")
        .set((workers * WORKER_QUEUE) as f64);
    let mut tasks = JoinSet::new();
    let mut queues = Vec::with_capacity(workers);
    for _ in 0..workers {
        let (queue, commands) = queue::channel(WORKER_QUEUE, inventory.clone());
        queues.push(queue);
        tasks.spawn(spawn(commands));
    }
    let mut closing = false;
    loop {
        tokio::select! {
            biased;
            _ = &mut stop, if !closing => { closing = true; receiver.close(); }
            command = receiver.recv() => {
                let Some(command) = command else { break; };
                let index = route(&command);
                if let Err(SendError(command)) = queues[index].send(command).await {
                    // The receiver is gone, and with it every command still queued
                    // behind the one that stopped the worker; only this one returns.
                    tracing::error!(target: "cmaeg::delivery::engine", worker = index, "delivery worker stopped; buffered commands lost");
                    metrics::counter!(DELIVERY_WORKER_RESTARTS).increment(1);
                    let (queue, commands) = queue::channel(WORKER_QUEUE, inventory.clone());
                    tasks.spawn(spawn(commands));
                    queues[index] = queue;
                    if queues[index].send(command).await.is_err() {
                        tracing::error!(target: "cmaeg::delivery::engine", worker = index, "delivery command dropped after worker replacement failed");
                    }
                }
            }
            Some(result) = tasks.join_next(), if !tasks.is_empty() => {
                // A panic payload can carry message content, so only its kind is logged.
                if let Err(error) = result {
                    tracing::error!(target: "cmaeg::delivery::engine", panicked = error.is_panic(), "delivery worker task ended");
                }
            }
        }
    }
    drop(queues);
    while let Some(result) = tasks.join_next().await {
        if result.is_err() {
            tracing::error!(target: "cmaeg::delivery::engine", "delivery worker stopped unexpectedly");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::dispatch;
    use metrics_exporter_prometheus::PrometheusBuilder;
    use std::{
        collections::BTreeSet,
        future::Future,
        io::Write,
        pin::Pin,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc, Mutex,
        },
        time::Duration,
    };
    use tokio::sync::{mpsc, oneshot, Notify};

    const PANICS: u32 = 13;
    const PAYLOAD: &str = "WORKER-PANIC-PAYLOAD";

    /// Owns a worker's receiver; the signal means the receiver is already gone.
    struct ReceiverProbe {
        rx: Option<super::queue::Receiver<u32>>,
        closed: Arc<Notify>,
    }
    impl Drop for ReceiverProbe {
        fn drop(&mut self) {
            drop(self.rx.take());
            self.closed.notify_one();
        }
    }

    /// Workers that record `(spawn ordinal, command)` and acknowledge each
    /// command; command 13 panics before it is recorded.
    #[derive(Clone)]
    struct Workers {
        recorded: Arc<Mutex<Vec<(usize, u32)>>>,
        acks: mpsc::UnboundedSender<u32>,
        closed: Arc<Notify>,
        spawned: Arc<AtomicUsize>,
    }
    impl Workers {
        fn new() -> (Self, mpsc::UnboundedReceiver<u32>) {
            let (acks, acknowledged) = mpsc::unbounded_channel();
            let workers = Self {
                recorded: Arc::default(),
                acks,
                closed: Arc::default(),
                spawned: Arc::default(),
            };
            (workers, acknowledged)
        }
        fn spawn(
            &self,
            rx: super::queue::Receiver<u32>,
        ) -> impl Future<Output = ()> + Send + 'static {
            let ordinal = self.spawned.fetch_add(1, Ordering::SeqCst);
            let mut probe = ReceiverProbe {
                rx: Some(rx),
                closed: self.closed.clone(),
            };
            let (recorded, acks) = (self.recorded.clone(), self.acks.clone());
            async move {
                while let Some(command) = probe.rx.as_mut().unwrap().recv().await {
                    assert_ne!(command, PANICS, "{PAYLOAD}");
                    recorded.lock().unwrap().push((ordinal, command));
                    acks.send(command).unwrap();
                }
            }
        }
        fn recorded(&self) -> Vec<(usize, u32)> {
            self.recorded.lock().unwrap().clone()
        }
    }

    fn route(command: &u32) -> usize {
        *command as usize % 2
    }

    async fn acknowledged(
        acks: &mut mpsc::UnboundedReceiver<u32>,
        expected: impl IntoIterator<Item = u32>,
    ) {
        let mut pending: BTreeSet<u32> = expected.into_iter().collect();
        tokio::time::timeout(Duration::from_secs(5), async {
            while !pending.is_empty() {
                pending.remove(&acks.recv().await.unwrap());
            }
        })
        .await
        .expect("commands were not acknowledged");
    }

    #[derive(Clone, Default)]
    struct Logs(Arc<Mutex<Vec<u8>>>);
    impl Write for Logs {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Runs `test` on this thread with local metrics and logs, returning both rendered.
    fn observe(test: impl Future<Output = ()>) -> (String, String) {
        let recorder = PrometheusBuilder::new().build_recorder();
        let metrics = recorder.handle();
        let logs = Logs::default();
        let writer = logs.clone();
        let _subscriber = tracing::subscriber::set_default(
            tracing_subscriber::fmt()
                .with_ansi(false)
                .without_time()
                .with_writer(move || writer.clone())
                .finish(),
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        metrics::with_local_recorder(&recorder, || runtime.block_on(test));
        let logs = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
        (metrics.render(), logs)
    }

    #[test]
    fn replaced_worker_receives_commands_after_a_panic() {
        let (workers, mut acks) = Workers::new();
        let observed = workers.clone();
        let (metrics, logs) = observe(async move {
            let (inbound, receiver) = mpsc::channel(64);
            let (_stop, stop) = oneshot::channel();
            let factory = workers.clone();
            let running = tokio::spawn(dispatch(receiver, stop, 2, route, move |rx| {
                factory.spawn(rx)
            }));
            for command in 1..=PANICS {
                inbound.send(command).await.unwrap();
            }
            // Commands queued behind 13 would be lost with its worker, so the
            // next ones are sent only once that worker's receiver is gone.
            acknowledged(&mut acks, 1..PANICS).await;
            tokio::time::timeout(Duration::from_secs(5), workers.closed.notified())
                .await
                .expect("the panicking worker kept its receiver");
            for command in 14..=20 {
                inbound.send(command).await.unwrap();
            }
            acknowledged(&mut acks, 14..=20).await;
            assert!(
                !running.is_finished(),
                "dispatch stopped after a worker panic"
            );
            drop(inbound);
            tokio::time::timeout(Duration::from_secs(5), running)
                .await
                .unwrap()
                .unwrap();
        });
        let recorded = observed.recorded();
        assert!(recorded.iter().all(|(_, command)| *command != PANICS));
        for command in 14..=20 {
            assert!(
                recorded.iter().any(|(_, seen)| *seen == command),
                "{command} was not delivered"
            );
        }
        // Workers 0 and 1 start with dispatch; 2 replaces the panicked index 1.
        for command in [15, 17, 19] {
            assert!(
                recorded.contains(&(2, command)),
                "{command} missed the replacement: {recorded:?}"
            );
        }
        assert_eq!(observed.spawned.load(Ordering::SeqCst), 3);
        assert!(
            metrics.contains("cmaeg_delivery_worker_restarts_total 1\n"),
            "{metrics}"
        );
        assert!(
            logs.contains("delivery worker stopped; buffered commands lost worker=1"),
            "{logs}"
        );
        assert!(
            logs.contains("delivery worker task ended panicked=true"),
            "{logs}"
        );
        assert!(!logs.contains(PAYLOAD), "{logs}");
    }

    #[test]
    fn dispatch_ends_only_when_its_inbound_queue_closes_or_stop_fires() {
        let (workers, mut acks) = Workers::new();
        let observed = workers.clone();
        observe(async move {
            let (inbound, receiver) = mpsc::channel(64);
            let (_stop, stop) = oneshot::channel();
            let factory = workers.clone();
            let mut running = tokio::spawn(dispatch(receiver, stop, 2, route, move |rx| {
                factory.spawn(rx)
            }));
            inbound.send(2).await.unwrap();
            acknowledged(&mut acks, [2]).await;
            let idle = tokio::time::timeout(Duration::from_millis(50), &mut running).await;
            assert!(
                idle.is_err(),
                "dispatch ended while its inbound queue was open"
            );
            drop(inbound);
            tokio::time::timeout(Duration::from_secs(5), running)
                .await
                .unwrap()
                .unwrap();

            let (inbound, receiver) = mpsc::channel(64);
            let (stop_sender, stop) = oneshot::channel();
            let factory = workers.clone();
            let running = tokio::spawn(dispatch(receiver, stop, 2, route, move |rx| {
                factory.spawn(rx)
            }));
            inbound.send(4).await.unwrap();
            inbound.send(5).await.unwrap();
            stop_sender.send(()).unwrap();
            tokio::time::timeout(Duration::from_secs(5), running)
                .await
                .unwrap()
                .unwrap();
            assert!(inbound.is_closed(), "stop left the inbound queue open");
        });
        // Commands accepted before stop are still delivered while workers drain.
        let commands: Vec<u32> = observed
            .recorded()
            .into_iter()
            .map(|(_, command)| command)
            .collect();
        assert_eq!(commands.len(), 3, "{commands:?}");
        assert!(
            [2, 4, 5].iter().all(|command| commands.contains(command)),
            "{commands:?}"
        );
    }

    #[test]
    fn a_replacement_that_also_stops_drops_the_command_and_dispatch_continues() {
        type Worker = Pin<Box<dyn Future<Output = ()> + Send>>;
        let (workers, mut acks) = Workers::new();
        let observed = workers.clone();
        let (metrics, logs) = observe(async move {
            let (inbound, receiver) = mpsc::channel(64);
            let (_stop, stop) = oneshot::channel();
            let spawned = Arc::new(AtomicUsize::new(0));
            // Only the first worker (index 0) runs; every later one, index 1
            // and each replacement, drops its receiver before it is returned.
            let factory = move |rx: super::queue::Receiver<u32>| -> Worker {
                if spawned.fetch_add(1, Ordering::SeqCst) == 0 {
                    Box::pin(workers.spawn(rx))
                } else {
                    drop(rx);
                    Box::pin(async {})
                }
            };
            let running = tokio::spawn(dispatch(receiver, stop, 2, route, factory));
            for command in 1..=4 {
                inbound.send(command).await.unwrap();
            }
            acknowledged(&mut acks, [2, 4]).await;
            assert!(
                !running.is_finished(),
                "dispatch stopped after a failed replacement"
            );
            drop(inbound);
            tokio::time::timeout(Duration::from_secs(5), running)
                .await
                .unwrap()
                .unwrap();
        });
        assert_eq!(observed.recorded(), [(0, 2), (0, 4)]);
        assert!(
            metrics.contains("cmaeg_delivery_worker_restarts_total 2\n"),
            "{metrics}"
        );
        assert_eq!(
            logs.matches("delivery command dropped after worker replacement failed worker=1")
                .count(),
            2,
            "{logs}"
        );
    }
}
