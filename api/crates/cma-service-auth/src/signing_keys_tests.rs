use super::*;
use std::{future::poll_fn, task::Poll};
use tokio::sync::{mpsc, oneshot};

const A: &str = "https://a.example/keys";
const B: &str = "https://b.example/keys";
#[derive(Debug, PartialEq, Eq)]
struct Keys(Vec<String>);
impl KeySet for Keys {
    fn contains(&self, kid: &str) -> bool {
        self.0.iter().any(|key| key == kid)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
enum Error {
    Parse,
    Transport(TransportFailure),
}
fn parse(bytes: &[u8]) -> Result<Keys, Error> {
    let text = std::str::from_utf8(bytes).map_err(|_| Error::Parse)?;
    if text.is_empty() {
        return Err(Error::Parse);
    }
    Ok(Keys(text.split(',').map(str::to_owned).collect()))
}
struct Request {
    url: String,
    reply: oneshot::Sender<Result<Vec<u8>, TransportFailure>>,
}
impl Request {
    fn keys(self, keys: &str) {
        self.reply.send(Ok(keys.as_bytes().to_vec())).unwrap();
    }
    fn fail(self) {
        self.reply.send(Err(TransportFailure::Status)).unwrap();
    }
}
struct Gated(mpsc::UnboundedSender<Request>);
impl Fetcher for Gated {
    fn fetch<'a>(&'a self, url: &'a str, _: usize) -> FetchFuture<'a> {
        Box::pin(async move {
            let (reply, response) = oneshot::channel();
            self.0
                .send(Request {
                    url: url.to_owned(),
                    reply,
                })
                .unwrap();
            response.await.expect("test fetch response")
        })
    }
}
struct Fixture {
    cache: Cache<Keys, Error>,
    requests: mpsc::UnboundedReceiver<Request>,
}
impl Fixture {
    fn new() -> Self {
        Self::configured(Config::new(Duration::from_secs(60), 4096))
    }
    fn configured(config: Config) -> Self {
        let (sender, requests) = mpsc::unbounded_channel();
        let cache = Cache::with_fetcher(config, parse, Error::Transport, Arc::new(Gated(sender)));
        Self { cache, requests }
    }
    async fn warm(&mut self, url: &str) {
        let lookup = self.cache.keys_for(url, "old");
        tokio::pin!(lookup);
        pending(lookup.as_mut()).await;
        let request = self.requests.recv().await.unwrap();
        assert_eq!(request.url, url);
        request.keys("old");
        assert!(lookup.await.unwrap().contains("old"));
    }
}
async fn pending<F: Future>(mut future: Pin<&mut F>) {
    poll_fn(|cx| {
        assert!(future.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
}
async fn ready<F: Future>(future: F) -> F::Output {
    tokio::pin!(future);
    poll_fn(|cx| match future.as_mut().poll(cx) {
        Poll::Ready(result) => Poll::Ready(result),
        Poll::Pending => panic!("cached lookup must resolve immediately"),
    })
    .await
}

#[tokio::test(start_paused = true)]
async fn cached_keys_resolve_immediately_during_unknown_key_refresh() {
    let mut f = Fixture::new();
    f.warm(A).await;
    tokio::time::advance(Duration::from_secs(5)).await;
    let miss = f.cache.keys_for(A, "unknown");
    tokio::pin!(miss);
    pending(miss.as_mut()).await;
    let request = f.requests.recv().await.unwrap();
    assert!(ready(f.cache.keys_for(A, "old"))
        .await
        .unwrap()
        .contains("old"));
    request.keys("old");
    assert_eq!(miss.await, Err(Outcome::UnknownKey));
}
#[tokio::test(start_paused = true)]
async fn concurrent_misses_share_one_refresh_even_after_cooldown_elapses() {
    for past_cooldown in [false, true] {
        let mut f = Fixture::new();
        f.warm(A).await;
        for _ in 0..20 {
            assert_eq!(
                ready(f.cache.keys_for(A, "unknown")).await,
                Err(Outcome::UnknownKey)
            );
        }
        assert!(f.requests.try_recv().is_err());
        tokio::time::advance(Duration::from_secs(5)).await;
        let starter = f.cache.keys_for(A, "unknown");
        tokio::pin!(starter);
        pending(starter.as_mut()).await;
        let request = f.requests.recv().await.unwrap();
        if past_cooldown {
            tokio::time::advance(Duration::from_secs(6)).await;
        }
        let kid = if past_cooldown { "rotated" } else { "unknown" };
        let mut joiners: Vec<_> = (0..20)
            .map(|_| Box::pin(f.cache.keys_for(A, kid)))
            .collect();
        for joiner in &mut joiners {
            pending(joiner.as_mut()).await;
        }
        tokio::task::yield_now().await;
        assert!(f.requests.try_recv().is_err());
        request.keys("old,rotated");
        assert_eq!(starter.await, Err(Outcome::UnknownKey));
        for joiner in joiners {
            if past_cooldown {
                assert!(joiner.await.unwrap().contains("rotated"));
            } else {
                assert_eq!(joiner.await, Err(Outcome::UnknownKey));
            }
        }
        assert!(f.requests.try_recv().is_err());
    }
}
#[tokio::test(start_paused = true)]
async fn failed_refresh_preserves_fresh_keys_and_joiners_recheck_cooldown() {
    for warm in [false, true] {
        let mut f = Fixture::new();
        if warm {
            f.warm(A).await;
            tokio::time::advance(Duration::from_secs(5)).await;
        }
        let starter = f.cache.keys_for(A, "unknown");
        let joiner = f.cache.keys_for(A, "unknown");
        tokio::pin!(starter, joiner);
        pending(starter.as_mut()).await;
        let request = f.requests.recv().await.unwrap();
        pending(joiner.as_mut()).await;
        if warm {
            assert!(ready(f.cache.keys_for(A, "old")).await.is_ok());
        }
        request.fail();
        assert_eq!(
            starter.await,
            Err(Outcome::Refresh(Error::Transport(TransportFailure::Status)))
        );
        let expected = if warm {
            Outcome::UnknownKey
        } else {
            Outcome::Throttled
        };
        assert_eq!(joiner.await, Err(expected.clone()));
        assert_eq!(ready(f.cache.keys_for(A, "unknown")).await, Err(expected));
        if warm {
            assert!(ready(f.cache.keys_for(A, "old")).await.is_ok());
        }
        assert!(f.requests.try_recv().is_err());
    }
}
#[tokio::test(start_paused = true)]
async fn rotated_keys_work_and_expired_keys_cannot_survive_a_failure() {
    let mut f = Fixture::new();
    f.warm(A).await;
    tokio::time::advance(Duration::from_secs(5)).await;
    let rotation = f.cache.keys_for(A, "new");
    tokio::pin!(rotation);
    pending(rotation.as_mut()).await;
    f.requests.recv().await.unwrap().keys("old,new");
    assert!(rotation.await.unwrap().contains("new"));
    assert!(ready(f.cache.keys_for(A, "old")).await.is_ok());
    tokio::time::advance(Duration::from_secs(60)).await;
    let refresh = f.cache.keys_for(A, "new");
    tokio::pin!(refresh);
    pending(refresh.as_mut()).await;
    let request = f.requests.recv().await.unwrap();
    let old = f.cache.keys_for(A, "old");
    tokio::pin!(old);
    pending(old.as_mut()).await;
    request.fail();
    assert!(matches!(refresh.await, Err(Outcome::Refresh(_))));
    assert_eq!(old.await, Err(Outcome::Throttled));
    tokio::time::advance(Duration::from_secs(5)).await;
    let refresh = f.cache.keys_for(A, "new");
    tokio::pin!(refresh);
    pending(refresh.as_mut()).await;
    f.requests.recv().await.unwrap().keys("new");
    assert!(refresh.await.unwrap().contains("new"));
    assert_eq!(
        ready(f.cache.keys_for(A, "old")).await,
        Err(Outcome::UnknownKey)
    );
}
#[tokio::test(start_paused = true)]
async fn cancelling_starter_does_not_cancel_or_duplicate_refresh() {
    let mut f = Fixture::new();
    let mut starter = Box::pin(f.cache.keys_for(A, "old"));
    pending(starter.as_mut()).await;
    let request = f.requests.recv().await.unwrap();
    drop(starter);
    tokio::time::advance(Duration::from_secs(6)).await;
    let joiner = f.cache.keys_for(A, "new");
    tokio::pin!(joiner);
    pending(joiner.as_mut()).await;
    assert!(f.requests.try_recv().is_err());
    request.keys("new");
    assert!(joiner.await.unwrap().contains("new"));
    assert!(ready(f.cache.keys_for(A, "new")).await.is_ok());
}
#[tokio::test(start_paused = true)]
async fn lost_refresh_task_is_unavailable_and_releases_its_flight() {
    let mut f = Fixture::new();
    {
        let lookup = f.cache.keys_for(A, "old");
        tokio::pin!(lookup);
        pending(lookup.as_mut()).await;
        // Dropping the reply makes the fake panic inside the independent refresh task.
        drop(f.requests.recv().await.unwrap());
        assert_eq!(lookup.await, Err(Outcome::FlightLost));
    }
    assert_eq!(
        ready(f.cache.keys_for(A, "old")).await,
        Err(Outcome::Throttled)
    );
    tokio::time::advance(Duration::from_secs(5)).await;
    f.warm(A).await;
}
#[tokio::test(start_paused = true)]
async fn dropped_sender_clears_only_its_own_flight() {
    let f = Fixture::new();
    let entry = f.cache.entry(A).unwrap();
    for replace in [false, true] {
        let (sender, receiver) = watch::channel(None);
        entry.lock().unwrap().flight = Some(receiver);
        let lookup = f.cache.keys_for(A, "old");
        tokio::pin!(lookup);
        pending(lookup.as_mut()).await;
        let (_new_sender, new_receiver) = watch::channel(None);
        if replace {
            entry.lock().unwrap().flight = Some(new_receiver.clone());
        }
        drop(sender);
        assert_eq!(lookup.await, Err(Outcome::FlightLost));
        let cache = entry.lock().unwrap();
        if replace {
            assert!(cache.flight.as_ref().unwrap().same_channel(&new_receiver));
        } else {
            assert!(cache.flight.is_none());
        }
    }
}
#[tokio::test(start_paused = true)]
async fn endpoints_refresh_independently_and_keep_cached_hits_available() {
    let mut f = Fixture::new();
    f.warm(A).await;
    f.warm(B).await;
    tokio::time::advance(Duration::from_secs(5)).await;
    let a = f.cache.keys_for(A, "new");
    tokio::pin!(a);
    pending(a.as_mut()).await;
    let a_request = f.requests.recv().await.unwrap();
    assert_eq!(a_request.url, A);
    assert!(ready(f.cache.keys_for(B, "old")).await.is_ok());
    let b = f.cache.keys_for(B, "new");
    tokio::pin!(b);
    pending(b.as_mut()).await;
    let b_request = f.requests.recv().await.unwrap();
    assert_eq!(b_request.url, B);
    b_request.keys("new");
    assert!(b.await.is_ok());
    pending(a.as_mut()).await;
    a_request.keys("new");
    assert!(a.await.is_ok());
}
#[tokio::test(start_paused = true)]
async fn capacity_preserves_live_entries_flights_cooldowns_and_fresh_keys() {
    let f = Fixture::new();
    let entries: Vec<_> = (0..4096)
        .map(|i| {
            f.cache
                .entry(&format!("https://issuer-{i}.example/keys"))
                .unwrap()
        })
        .collect();
    assert!(matches!(f.cache.entry(A), Err(Outcome::Capacity)));
    let (sender, receiver) = watch::channel(None);
    for entry in &entries {
        entry.lock().unwrap().flight = Some(receiver.clone());
    }
    drop(entries);
    assert!(matches!(f.cache.entry(A), Err(Outcome::Capacity)));
    for entry in f.cache.entries.lock().unwrap().values() {
        let mut cache = entry.lock().unwrap();
        cache.flight = None;
        cache.next_refresh = Some(Instant::now() + Duration::from_secs(5));
    }
    drop(sender);
    assert!(matches!(f.cache.entry(A), Err(Outcome::Capacity)));
    tokio::time::advance(Duration::from_secs(5)).await;
    for entry in f.cache.entries.lock().unwrap().values() {
        entry.lock().unwrap().keys = Some((
            Arc::new(parse(b"old").unwrap()),
            Instant::now() + Duration::from_secs(60),
        ));
    }
    assert!(matches!(f.cache.entry(A), Err(Outcome::Capacity)));
    tokio::time::advance(Duration::from_secs(60)).await;
    assert!(f.cache.entry(A).is_ok());
    assert_eq!(f.cache.entries.lock().unwrap().len(), 1);
}
#[tokio::test(start_paused = true)]
async fn parser_and_transport_failures_keep_the_callers_error_vocabulary() {
    for failure in [
        TransportFailure::Request,
        TransportFailure::Status,
        TransportFailure::Bounds,
        TransportFailure::Body,
    ] {
        let mut f = Fixture::new();
        let lookup = f.cache.keys_for(A, "old");
        tokio::pin!(lookup);
        pending(lookup.as_mut()).await;
        f.requests
            .recv()
            .await
            .unwrap()
            .reply
            .send(Err(failure))
            .unwrap();
        assert_eq!(
            lookup.await,
            Err(Outcome::Refresh(Error::Transport(failure)))
        );
    }
    for (bytes, error) in [
        (Vec::new(), Error::Parse),
        (vec![0; 33], Error::Transport(TransportFailure::Bounds)),
    ] {
        let mut config = Config::new(Duration::from_secs(60), 1);
        config.max_body_bytes = 32;
        let mut f = Fixture::configured(config);
        let lookup = f.cache.keys_for(A, "old");
        tokio::pin!(lookup);
        pending(lookup.as_mut()).await;
        f.requests
            .recv()
            .await
            .unwrap()
            .reply
            .send(Ok(bytes))
            .unwrap();
        assert_eq!(lookup.await, Err(Outcome::Refresh(error)));
    }
}
