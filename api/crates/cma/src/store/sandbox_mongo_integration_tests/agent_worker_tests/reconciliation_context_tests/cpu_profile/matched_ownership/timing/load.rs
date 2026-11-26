use super::*;
use std::{sync::mpsc, thread};
use tokio::sync::oneshot;

pub(super) struct HttpRead {
    pub(super) start: Duration,
    pub(super) end: Duration,
    pub(super) scheduled_ms: u64,
    pub(super) handler_ms: f64,
}

impl HttpRead {
    pub(super) fn latency_ms(&self) -> f64 {
        (self.end - self.start).as_secs_f64() * 1000.
    }

    pub(super) fn dispatch_delay_ms(&self) -> f64 {
        self.start
            .saturating_sub(Duration::from_millis(self.scheduled_ms))
            .as_secs_f64()
            * 1000.
    }
}

type Window = (Vec<HttpRead>, BTreeMap<u16, usize>);

struct Job {
    started: Instant,
    phase_ms: u64,
    result: oneshot::Sender<Window>,
}

// The request producer, including its timers and HTTP client, has a separate
// thread and Tokio executor. It cannot consume the managers' worker threads.
pub(in super::super) struct LoadGenerator {
    jobs: Option<mpsc::Sender<Job>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl LoadGenerator {
    pub(in super::super) fn start(servers: &[HttpForeground], target: AgentId) -> Self {
        let addresses: Vec<_> = servers
            .iter()
            .map(|server| server.address.clone())
            .collect();
        let (jobs, pending) = mpsc::channel::<Job>();
        let (ready, started) = mpsc::sync_channel(0);
        let thread = thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let client = runtime.block_on(async {
                reqwest::Client::builder()
                    .timeout(Duration::from_secs(3))
                    .build()
                    .unwrap()
            });
            ready.send(()).unwrap();
            while let Ok(job) = pending.recv() {
                let result = runtime.block_on(read_window(
                    &client,
                    &addresses,
                    &target,
                    job.started,
                    job.phase_ms,
                ));
                assert!(job.result.send(result).is_ok());
            }
        });
        started.recv().unwrap();
        Self {
            jobs: Some(jobs),
            thread: Some(thread),
        }
    }

    pub(super) async fn window(&self, started: Instant, foreground: bool, phase_ms: u64) -> Window {
        if !foreground {
            return (Vec::new(), BTreeMap::new());
        }
        let (result, complete) = oneshot::channel();
        self.jobs
            .as_ref()
            .unwrap()
            .send(Job {
                started,
                phase_ms,
                result,
            })
            .unwrap();
        complete.await.unwrap()
    }
}

impl Drop for LoadGenerator {
    fn drop(&mut self) {
        drop(self.jobs.take());
        self.thread.take().unwrap().join().unwrap();
    }
}

async fn read_window(
    client: &reqwest::Client,
    addresses: &[String],
    target: &AgentId,
    started: Instant,
    phase_ms: u64,
) -> Window {
    let mut reads = Vec::new();
    let mut statuses = BTreeMap::new();
    for index in 0..64 {
        let scheduled_ms = phase_ms + index * 125;
        tokio::time::sleep_until((started + Duration::from_millis(scheduled_ms)).into()).await;
        let start = started.elapsed();
        let address = &addresses[index as usize % addresses.len()];
        let response = client
            .get(format!("{address}/api/v1/agents/{target}"))
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        let handler_ms = response.headers()["x-fixture-handler-us"]
            .to_str()
            .unwrap()
            .parse::<f64>()
            .unwrap()
            / 1000.;
        let body = response.bytes().await.unwrap();
        reads.push(HttpRead {
            start,
            end: started.elapsed(),
            scheduled_ms,
            handler_ms,
        });
        *statuses.entry(status).or_insert(0) += 1;
        if status == 200 {
            let value: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(value["id"], target.as_str());
        }
    }
    (reads, statuses)
}
