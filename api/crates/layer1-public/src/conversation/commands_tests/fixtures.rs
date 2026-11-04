use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Method, Request},
    Router,
};
use layer2_core::{
    sandbox::{
        agent_io::*,
        store::{InMemorySandboxStore, PageRequest},
        *,
    },
    AgentId, RuntimeCapabilities, RuntimeCapability,
};
use serde_json::Value;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use crate::{rest::agents_tests as fixtures, AgentConversationState};

pub(super) const ID: &str = "agt_protocol";
pub(super) const AGUI: &str = "/api/v1/agui/run";
pub(super) const RESPONSES: &str = "/api/v1/responses";

pub(super) use crate::conversation::command_test_support::context;

#[derive(Default)]
pub(super) struct Runtime(
    pub(super) Arc<Mutex<Vec<String>>>,
    pub(super) Arc<AtomicBool>,
);
#[async_trait]
impl AgentRuntimeRegistry for Runtime {
    async fn resolve(
        &self,
        _: &AgentRuntimeTarget,
    ) -> Result<AgentRuntimeConnection, AgentRuntimeError> {
        let calls = self.0.clone();
        let ambiguous = self.1.clone();
        Ok(AgentRuntimeConnection {
            capabilities: RuntimeCapabilities::new([
                RuntimeCapability::Conversation,
                RuntimeCapability::Steer,
                RuntimeCapability::Interrupt,
            ]),
            input: AgentRuntimeInputSender::spawn(8, move |input| {
                calls.lock().unwrap().push(format!("{input:?}"));
                let fail = ambiguous.load(Ordering::SeqCst);
                async move {
                    if fail {
                        Err(AgentRuntimeInputError::Unavailable("reply lost".into()))
                    } else {
                        Ok(())
                    }
                }
            }),
        })
    }
    async fn fence(&self, _: &AgentId, _: u64) -> Result<(), AgentRuntimeError> {
        Ok(())
    }
}

pub(super) struct Fixture {
    pub router: Router,
    pub state: AgentConversationState,
    pub store: Arc<InMemorySandboxStore>,
    pub runtime: Arc<Runtime>,
}
pub(super) async fn fixture(working: bool) -> Fixture {
    let runtime = Arc::new(Runtime::default());
    let (mut state, store) = fixtures::conversation_fixture_with_registry(runtime.clone()).await;
    state = crate::conversation::command_test_support::compose(state, store.clone());
    fixtures::insert_agent(
        &store,
        fixtures::seeded_agent(ID, AgentStatus::Ready, 2, None),
        vec![],
    )
    .await;
    if working {
        state
            .operations
            .send_message(
                &context("owner-a").caller,
                &AgentId(ID.into()),
                "Earlier turn".into(),
            )
            .await
            .unwrap();
        runtime.0.lock().unwrap().clear();
    }
    let router = crate::agui::router(state.clone())
        .merge(crate::responses::router(state.clone()))
        .merge(crate::responses::v2::router(
            crate::responses::v2::ResponsesV2State::new(
                crate::agent_page::AgentPageOrigin::test(),
                state.commands.clone().unwrap(),
                state.reads.clone(),
            ),
        ));
    Fixture {
        router,
        state,
        store,
        runtime,
    }
}

pub(super) fn request(path: &str, key: Option<&str>, body: Value) -> Request<Body> {
    let mut request = fixtures::request(Method::POST, path, "owner-a", key, Some(body));
    request.extensions_mut().insert(context("owner-a"));
    request
        .extensions_mut()
        .insert(crate::DelegatedCredential::new("fixture-credential".into()));
    request
}

pub(super) async fn admitted(f: &Fixture) -> Vec<AdmittedAgentCommand> {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(500);
    loop {
        let records = f
            .store
            .command_work_page(&PageRequest::default())
            .await
            .unwrap()
            .items;
        if !records.is_empty() || tokio::time::Instant::now() >= deadline {
            return records;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}
