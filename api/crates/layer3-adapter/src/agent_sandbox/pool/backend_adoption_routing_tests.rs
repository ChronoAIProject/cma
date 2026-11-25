use super::*;
use crate::hosted::{RuntimeBackends, RuntimeRoutingStore};
use std::collections::BTreeMap;

struct NoAllocation;
#[async_trait::async_trait]
impl RuntimeRoutingStore for NoAllocation {
    async fn select(&self, _: &str, _: u64, _: bool) -> Result<bool> {
        panic!("adoption must retain the recorded provider")
    }
}

#[tokio::test]
async fn adapter_and_router_adopt_with_home_and_keep_both_outer_home_proofs() {
    let f = inspection_fixture(true).await;
    let count = || {
        let mut counts = BTreeMap::new();
        for (call, _) in &f.bound.fixture.state.lock().unwrap().calls {
            assert!(call.starts_with("GET "));
            *counts.entry(call.clone()).or_insert(0usize) += 1;
        }
        counts
    };
    f.backend
        .adopt_with_home(&f.bound.runtime, &f.bound.request, &f.home)
        .await
        .unwrap()
        .unwrap();
    let mut expected = count();
    let claim = format!(
        "GET {}/sandboxclaims/{}",
        crate::agent_sandbox::pool::fixture::prefix(),
        f.bound.claim.name
    );
    // CodexAdapter must still prove the persisted home graph before and after
    // backend adoption. The router forwards identity, never selects a new owner.
    for path in [claim, format!("GET {MEMBER}"), format!("GET {VOLUME}")] {
        *expected.get_mut(&path).unwrap() += 2;
    }
    let homes = Arc::new(
        RoutedHomeVolumes::new(
            None,
            Some((
                Arc::new(
                    NativeHomeVolumes::new(
                        f.bound.fixture.kubernetes.clone(),
                        f.bound.fixture.client.config.clone(),
                        "fixture",
                    )
                    .unwrap(),
                ),
                f.home.request.policy.clone(),
            )),
        )
        .unwrap(),
    );
    let router = Arc::new(RuntimeBackends {
        agent_sandbox: Some(f.backend.clone()),
        opensandbox: None,
        allocate_agent_sandbox: false,
        routing: Arc::new(NoAllocation),
        home_routes: Some(homes.clone()),
    });
    let config = super::super::super::super::tests::spec();
    let adapter = CodexAdapter::new(
        router,
        CodexSandboxSpec {
            deployment: HostedDeployment {
                image: config.image,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .with_home_volumes(homes);
    f.bound.fixture.state.lock().unwrap().calls.clear();
    assert!(adapter
        .adopt_runtime(&f.adoption)
        .await
        .unwrap()
        .unwrap()
        .lifecycle
        .is_running());
    assert_eq!(count(), expected);
    f.bound
        .fixture
        .state
        .lock()
        .unwrap()
        .rows
        .get_mut(VOLUME)
        .unwrap()["metadata"]["uid"] = json!("changed-home");
    assert!(adapter.adopt_runtime(&f.adoption).await.is_err());
    assert_eq!(f.transport.calls.load(Ordering::SeqCst), 0);
}
