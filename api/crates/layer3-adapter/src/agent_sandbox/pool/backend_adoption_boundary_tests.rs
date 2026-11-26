use super::*;
use crate::agent_sandbox::pool::read_fixture::Gate;

struct OnBind {
    inner: Arc<Transport>,
    callback: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}
#[async_trait::async_trait]
impl HostedControlTransport for OnBind {
    async fn bind_runtime(&self, handle: &RuntimeHandle, sandbox: &str) -> Result<()> {
        self.inner.bind_runtime(handle, sandbox).await?;
        if let Some(callback) = self.callback.lock().unwrap().take() {
            callback();
        }
        Ok(())
    }
    async fn exchange(&self, _: &RuntimeHandle, _: RequestFrame) -> Result<ResponseFrame> {
        panic!("adoption does not dispatch control")
    }
    async fn open_agent(
        &self,
        _: &RuntimeHandle,
        _: FencedRequest<AgentLaunch>,
        _: Option<String>,
        _: HostedAgentOpenSettings,
        _: tokio::sync::mpsc::Sender<ConversationUpdate>,
    ) -> Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        panic!("adoption does not open an Agent")
    }
    async fn retire(&self, _: &RuntimeHandle) -> Result<()> {
        panic!("adoption does not retire")
    }
}

#[tokio::test]
async fn persisted_home_adoption_rechecks_graph_and_policy_after_transport_binding() {
    for change in ["claim", "member", "volume", "policy"] {
        let bound = bound(true).await;
        let home = adopted(&bound.fixture, &bound.claim).await;
        let path = match change {
            "claim" => format!(
                "{}/sandboxclaims/{}",
                crate::agent_sandbox::pool::fixture::prefix(),
                bound.claim.name
            ),
            "member" => MEMBER.into(),
            "volume" => VOLUME.into(),
            "policy" => format!(
                "{}/sandboxtemplates/{}",
                crate::agent_sandbox::pool::fixture::prefix(),
                bound.claim.request.pool.template_name
            ),
            _ => unreachable!(),
        };
        let state = bound.fixture.state.clone();
        let transport = Arc::new(OnBind {
            inner: bound.transport.clone(),
            callback: Mutex::new(Some(Box::new(move || {
                let mut state = state.lock().unwrap();
                let row = state.rows.get_mut(&path).unwrap();
                if change == "policy" {
                    row["spec"]["networkPolicy"]["egress"] = json!([{}]);
                } else {
                    row["metadata"]["uid"] = json!("changed-after-bind");
                }
            }))),
        });
        let backend = AgentSandboxBackend::new(client(&bound.fixture, false), transport);
        assert!(
            backend
                .adopt_with_home(&bound.runtime, &bound.request, &home)
                .await
                .is_err(),
            "{change}"
        );
        assert_eq!(bound.fixture.state.lock().unwrap().assignments.len(), 2);
        assert_eq!(bound.transport.binds.lock().unwrap().len(), 2);
    }
}

#[tokio::test]
async fn persisted_home_adoption_waits_for_all_graph_proofs_and_cancels_without_effect() {
    let bound = bound(false).await;
    let home = adopted(&bound.fixture, &bound.claim).await;
    let paths = [
        format!(
            "{}/sandboxclaims/{}",
            crate::agent_sandbox::pool::fixture::prefix(),
            bound.claim.name
        ),
        MEMBER.into(),
        VOLUME.into(),
    ];
    let gates: BTreeMap<_, _> = paths
        .iter()
        .map(|path| (path.clone(), Gate::new()))
        .collect();
    let effects = Effects::new(&bound.fixture, gates.clone());
    let backend = AgentSandboxBackend::new(
        AgentSandboxClient::new(
            effects.client.clone(),
            bound.fixture.client.config.clone(),
            "fixture",
        )
        .unwrap(),
        bound.transport.clone(),
    );
    let handle = bound.runtime.clone();
    let requested = bound.request.clone();
    let task =
        tokio::spawn(async move { backend.adopt_with_home(&handle, &requested, &home).await });
    for gate in gates.values() {
        gate.wait().await;
    }
    assert_eq!(effects.calls.lock().unwrap().len(), 3);
    assert_eq!(bound.fixture.state.lock().unwrap().assignments.len(), 1);
    assert_eq!(bound.transport.binds.lock().unwrap().len(), 1);
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    for gate in gates.values() {
        gate.respond();
    }
    tokio::task::yield_now().await;
    effects.assert_read_only();
    assert_eq!(bound.fixture.state.lock().unwrap().assignments.len(), 1);
    assert_eq!(bound.transport.binds.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn persisted_home_adoption_pending_refresh_is_bounded_and_uses_fresh_graph() {
    for become_ready in [false, true] {
        let bound = bound(false).await;
        let home = adopted(&bound.fixture, &bound.claim).await;
        let state = bound.fixture.state.clone();
        let transport = Arc::new(OnBind {
            inner: bound.transport.clone(),
            callback: Mutex::new(Some(Box::new(move || {
                state.lock().unwrap().rows.get_mut(MEMBER).unwrap()["status"]["conditions"][0]
                    ["status"] = json!("False");
                if become_ready {
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        state.lock().unwrap().rows.get_mut(MEMBER).unwrap()["status"]
                            ["conditions"][0]["status"] = json!("True");
                    });
                }
            }))),
        });
        let backend = AgentSandboxBackend::new(client(&bound.fixture, false), transport);
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            backend.adopt_with_home(&bound.runtime, &bound.request, &home),
        )
        .await
        .unwrap();
        if become_ready {
            assert!(result.unwrap().unwrap().lifecycle.is_running());
        } else {
            assert!(
                matches!(result, Err(AdapterError::Unavailable(message)) if message == "Sandbox readiness is still pending")
            );
        }
        assert_eq!(bound.fixture.state.lock().unwrap().assignments.len(), 2);
        assert_eq!(bound.transport.binds.lock().unwrap().len(), 2);
    }
}
