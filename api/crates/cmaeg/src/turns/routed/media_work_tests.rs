use super::*;
use crate::turns::routed::{
    dispatcher::{DispatchOutcome, Dispatcher, ExecutionPort, StartResult},
    test_support::{fixture, supported, Port, Setup},
    RecoverySupport,
};
use crate::{
    channels::resolver::ResolvedChannel,
    ingress::routing_store::test_server::{Fault, Server},
};
use cmaeg_core::channel::{ProviderBinding, Secret};
use serde_json::json;
use std::sync::Arc;
struct IndexedPort {
    inner: Arc<Port>,
    index: MediaWorkIndex,
}
impl ExecutionPort for IndexedPort {
    fn support(&self, binding: &ProviderBinding) -> RecoverySupport {
        self.inner.support(binding)
    }
    fn prepare<'a>(
        &'a self,
        plan: &'a AdmittedPlan,
        target: &'a StoredTarget,
        current: &'a ResolvedChannel,
    ) -> BoxFuture<'a, Result<FrozenTurn, ExecutionError>> {
        Box::pin(async move {
            let mut body = self.inner.prepare(plan, target, current).await?;
            body.metadata.insert(
                super::super::media_schedule::KEY.into(),
                r#"{"v":1,"t":[],"x":[]}"#.into(),
            );
            Ok(body)
        })
    }
    fn prepare_continuations<'a>(
        &'a self,
        plan: &'a AdmittedPlan,
        target: &'a StoredTarget,
        body: &'a FrozenTurn,
    ) -> BoxFuture<'a, Result<(), ExecutionError>> {
        Box::pin(self.index.prepare(plan, target, body))
    }
    fn start<'a>(
        &'a self,
        body: &'a FrozenTurn,
        binding: &'a ProviderBinding,
        credential: &'a Secret,
    ) -> BoxFuture<'a, StartResult> {
        self.inner.start(body, binding, credential)
    }
}
#[tokio::test]
async fn media_index_failure_after_freeze_blocks_original_post_and_exact_retry_repairs_gap() {
    let setup = Setup::with_request(
        supported(),
        fixture::request(
            |value| {
                value["provider"]["target"] =
                    json!({"type":"existing_agent","agent_id":"agt_existing"});
            },
            fixture::message("media"),
        ),
    )
    .await;
    let index_server = Server::start().await;
    let index = MediaWorkIndex::fixture(
        setup.journal().clone(),
        setup.server.store(),
        index_server.records.clone(),
    );
    let port = Arc::new(IndexedPort {
        inner: setup.port.clone(),
        index: index.clone(),
    });
    let dispatcher = Dispatcher::new(
        setup.server.store(),
        setup.journal().clone(),
        setup.current.clone(),
        port,
    );
    *index_server.memory.fault.lock().unwrap() = Some(Fault {
        id: id(setup.target()),
        store: false,
        lose_read: false,
        skip: 0,
    });
    assert!(dispatcher
        .dispatch(&setup.plan.identity, &setup.target().id, "first")
        .await
        .is_err());
    assert!(setup.port.starts.lock().unwrap().is_empty());
    let (parent, _) = setup
        .journal()
        .read::<TargetState>(&setup.target().id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(parent.phase, Phase::Prepared);
    let body_hash = parent.body.unwrap().hash;
    let DispatchOutcome::Replay { mut lease, .. } = dispatcher
        .dispatch(&setup.plan.identity, &setup.target().id, "retry")
        .await
        .unwrap()
    else {
        panic!("replay");
    };
    assert_eq!(setup.port.starts.lock().unwrap().len(), 1);
    assert_eq!(lease.state().body.as_ref().unwrap().hash, body_hash);
    let head = index
        .load(&setup.plan, setup.target())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(head.frozen_body_sha256, body_hash);
    let page = index.page(None).await.unwrap();
    assert_eq!(page.plans.len(), 1);
    assert!(page.issues.is_empty());
    setup.journal().release(&mut lease).await.unwrap();
    let _ = dispatcher
        .dispatch(&setup.plan.identity, &setup.target().id, "accepted-replay")
        .await
        .unwrap();
    assert_eq!(setup.port.starts.lock().unwrap().len(), 1);
}
