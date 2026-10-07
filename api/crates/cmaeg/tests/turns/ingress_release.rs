//! How long a launched turn holds the shared ingress permit.
use super::{fake_provider::FakeProviderConfig, support::Fixture};
use cmaeg::{
    ingress::launch::{LaunchOutcome, LaunchRequest, TurnLauncher},
    TurnConfig,
};
use futures_util::future::BoxFuture;
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;

/// Records how many permits were free while `launch` ran.
struct Observer {
    semaphore: Arc<Semaphore>,
    seen: Mutex<Vec<usize>>,
}

impl TurnLauncher for Observer {
    fn launch(&self, _request: LaunchRequest) -> BoxFuture<'_, LaunchOutcome> {
        Box::pin(async move {
            tokio::task::yield_now().await;
            self.seen
                .lock()
                .unwrap()
                .push(self.semaphore.available_permits());
            LaunchOutcome::Deferred
        })
    }
}

#[tokio::test]
async fn default_launch_holding_keeps_the_permit_until_launch_returns() {
    let fixture = Fixture::new(FakeProviderConfig::default(), TurnConfig::default(), 1).await;
    let semaphore = Arc::new(Semaphore::new(2));
    let observer = Observer {
        semaphore: semaphore.clone(),
        seen: Mutex::default(),
    };
    let permit = semaphore.clone().try_acquire_owned().unwrap();
    assert_eq!(semaphore.available_permits(), 1);
    let outcome = observer
        .launch_holding(fixture.request("hold-default"), permit)
        .await;
    assert!(matches!(outcome, LaunchOutcome::Deferred));
    assert_eq!(*observer.seen.lock().unwrap(), [1], "held during launch");
    assert_eq!(semaphore.available_permits(), 2, "released after launch");
    fixture.fake.close().await;
}
