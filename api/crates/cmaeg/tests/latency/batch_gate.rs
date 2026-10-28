//! Hold the fake record read until every signed HTTP caller is awaiting restore.
//! No production resolver, signature verification or timing clock is replaced.
use axum::{
    body::{to_bytes, Body},
    extract::{Path, Request, State},
    middleware::from_fn,
    response::Response,
    routing::post,
    Router,
};
use cmaeg::{http, ingress::IngressState};
use std::{
    collections::BTreeSet,
    future::Future,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    task::Poll,
    time::Duration,
};
use tokio::sync::{Notify, Semaphore};

type ReadGate = Arc<Mutex<Option<Arc<Semaphore>>>>;

pub struct BatchGate {
    read: ReadGate,
    round: Mutex<Option<Arc<Round>>>,
}
struct Round {
    ids: Mutex<BTreeSet<String>>,
    expected: usize,
    arrived: AtomicUsize,
    changed: Notify,
    release: Arc<Semaphore>,
}
impl BatchGate {
    pub fn new(read: ReadGate) -> Arc<Self> {
        Arc::new(Self {
            read,
            round: Mutex::new(None),
        })
    }
    pub fn arm(&self, ids: impl IntoIterator<Item = String>) {
        let ids: BTreeSet<_> = ids.into_iter().collect();
        assert!(!ids.is_empty());
        let mut active = self.round.lock().unwrap();
        if let Some(previous) = active.as_ref() {
            assert_eq!(previous.arrived.load(Ordering::SeqCst), previous.expected);
        }
        let release = Arc::new(Semaphore::new(0));
        *self.read.lock().unwrap() = Some(release.clone());
        *active = Some(Arc::new(Round {
            expected: ids.len(),
            ids: Mutex::new(ids),
            arrived: AtomicUsize::new(0),
            changed: Notify::new(),
            release,
        }));
    }
    pub async fn joined(&self, count: usize) {
        let round = self.round.lock().unwrap().clone().unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let changed = round.changed.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                if round.arrived.load(Ordering::SeqCst) >= count {
                    break;
                }
                changed.await;
            }
        })
        .await
        .expect("cold batch callers reached the restore flight");
    }
    fn claim(&self, request: &Request) -> Option<Arc<Round>> {
        let id = request.headers().get("x-nyxid-message-id")?.to_str().ok()?;
        let round = self.round.lock().unwrap().clone()?;
        let claimed = round.ids.lock().unwrap().remove(id);
        claimed.then_some(round)
    }
    fn pending(&self, round: &Round) {
        if round.arrived.fetch_add(1, Ordering::SeqCst) + 1 == round.expected {
            self.read.lock().unwrap().take();
            round.release.add_permits(1);
        }
        round.changed.notify_waiters();
    }
}

#[derive(Clone)]
struct CallbackState {
    ingress: IngressState,
    gate: Arc<BatchGate>,
}

pub fn router(ingress: IngressState, gate: Arc<BatchGate>) -> Router {
    cmaeg::telemetry::http::instrument(
        Router::new()
            .route("/v1/callbacks/{channel_id}", post(callback))
            .with_state(CallbackState { ingress, gate })
            .layer(from_fn(http::problems::protocol_headers)),
    )
}

async fn callback(
    State(state): State<CallbackState>,
    Path(channel): Path<String>,
    request: Request,
) -> Response {
    let Some(round) = state.gate.claim(&request) else {
        return http::callbacks::callback(State(state.ingress), Path(channel), request).await;
    };
    // A complete in-memory body makes the real handler's first pending await
    // the cold resolver; network/body scheduling cannot impersonate a join.
    let (parts, body) = request.into_parts();
    let bytes = to_bytes(body, state.ingress.config.max_body).await.unwrap();
    let request = Request::from_parts(parts, Body::from(bytes));
    let mut pending = Box::pin(http::callbacks::callback(
        State(state.ingress),
        Path(channel),
        request,
    ));
    let first = futures_util::future::poll_fn(|cx| {
        // A cooperative-budget yield is not evidence of joining the resolver.
        let mut first_poll = std::pin::pin!(tokio::task::unconstrained(pending.as_mut()));
        Poll::Ready(first_poll.as_mut().poll(cx))
    })
    .await;
    assert!(
        first.is_pending(),
        "cold callback completed before the shared read was released"
    );
    state.gate.pending(&round);
    pending.await
}
