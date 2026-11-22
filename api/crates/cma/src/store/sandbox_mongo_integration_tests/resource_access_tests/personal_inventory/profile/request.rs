use super::identity::{Identity, Recorder};
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll, Wake, Waker},
    time::Duration,
};
use tokio::time::Instant;

#[derive(Clone, Debug, Default, serde::Serialize)]
pub(super) struct Sample {
    pub completed: bool,
    pub wall_us: u64,
    pub poll_us: u64,
    pub wake_to_poll_us: u64,
    pub polls: u64,
    pub woken_polls: u64,
    pub identity: Identity,
}

#[derive(Default)]
struct WakeState {
    parent: Option<Waker>,
    first_wake: Option<Instant>,
}

#[derive(Default)]
struct Observer(Mutex<WakeState>);

impl Wake for Observer {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        let parent = {
            let mut state = self.0.lock().unwrap();
            if state.parent.is_some() && state.first_wake.is_none() {
                state.first_wake = Some(Instant::now());
            }
            state.parent.clone()
        };
        if let Some(parent) = parent {
            parent.wake();
        }
    }
}

pub(super) struct Measured<F: Future> {
    future: Option<Pin<Box<F>>>,
    recorder: Recorder,
    observer: Arc<Observer>,
    output: Arc<Mutex<Option<Sample>>>,
    started: Option<Instant>,
    last_poll_end: Option<Instant>,
    poll_time: Duration,
    wake_delay: Duration,
    polls: u64,
    wakes: u64,
}

fn micros(duration: Duration) -> u64 {
    duration.as_micros().try_into().unwrap()
}

impl<F: Future> Measured<F> {
    pub(super) fn new(future: F, output: Arc<Mutex<Option<Sample>>>) -> Self {
        Self {
            future: Some(Box::pin(future)),
            recorder: Recorder::default(),
            observer: Arc::new(Observer::default()),
            output,
            started: None,
            last_poll_end: None,
            poll_time: Duration::ZERO,
            wake_delay: Duration::ZERO,
            polls: 0,
            wakes: 0,
        }
    }

    fn finish(&mut self, completed: bool) {
        metrics::with_local_recorder(&self.recorder, || drop(self.future.take()));
        self.observer.0.lock().unwrap().parent = None;
        *self.output.lock().unwrap() = Some(Sample {
            completed,
            wall_us: self.started.map_or(0, |start| micros(start.elapsed())),
            poll_us: micros(self.poll_time),
            wake_to_poll_us: micros(self.wake_delay),
            polls: self.polls,
            woken_polls: self.wakes,
            identity: self.recorder.0.lock().unwrap().clone(),
        });
    }
}

impl<F: Future> Future for Measured<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let now = Instant::now();
        this.started.get_or_insert(now);
        {
            let mut state = this.observer.0.lock().unwrap();
            state.parent = Some(cx.waker().clone());
            if let Some(wake) = state.first_wake.take() {
                let ready = wake.max(this.last_poll_end.unwrap_or(wake));
                this.wake_delay += now.saturating_duration_since(ready);
                this.wakes += 1;
            }
        }
        let waker = Waker::from(this.observer.clone());
        let mut observed = Context::from_waker(&waker);
        let started = Instant::now();
        let result = metrics::with_local_recorder(&this.recorder, || {
            this.future.as_mut().unwrap().as_mut().poll(&mut observed)
        });
        this.poll_time += started.elapsed();
        this.last_poll_end = Some(Instant::now());
        this.polls += 1;
        if result.is_ready() {
            this.finish(true);
        }
        result
    }
}

impl<F: Future> Drop for Measured<F> {
    fn drop(&mut self) {
        if self.future.is_some() {
            self.finish(false);
        }
    }
}
