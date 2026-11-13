//! Policy must be observed after the post-response graph, not before it.
use super::*;
use crate::agent_sandbox::pool::{
    fixture::prefix,
    read_fixture::{Effects, Gate},
};
use std::{collections::BTreeMap, time::Duration};
use tracing::{
    instrument::WithSubscriber,
    span::{Attributes, Id, Record},
    Event, Metadata, Subscriber,
};

#[derive(Clone, Default)]
struct Timing(Arc<Mutex<Vec<BTreeMap<String, serde_json::Value>>>>);

impl Subscriber for Timing {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }
    fn record(&self, _: &Id, _: &Record<'_>) {}
    fn record_follows_from(&self, _: &Id, _: &Id) {}
    fn enter(&self, _: &Id) {}
    fn exit(&self, _: &Id) {}
    fn event(&self, event: &Event<'_>) {
        #[derive(Default)]
        struct Fields(BTreeMap<String, serde_json::Value>);
        impl tracing::field::Visit for Fields {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                self.0
                    .insert(field.name().into(), json!(format!("{value:?}")));
            }
            fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
                self.0.insert(field.name().into(), json!(value));
            }
            fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
                self.0.insert(field.name().into(), json!(value));
            }
        }
        let mut fields = Fields::default();
        event.record(&mut fields);
        if fields
            .0
            .get("phase")
            .and_then(|v| v.as_str())
            .is_some_and(|v| matches!(v, "post_status_graph" | "post_status_policy"))
        {
            self.0.lock().unwrap().push(fields.0);
        }
    }
}

impl Timing {
    fn assert_phases(&self, expected: &[(&str, bool, bool)]) {
        let rows = self.0.lock().unwrap();
        assert_eq!(rows.len(), expected.len());
        for (row, (phase, completed, succeeded)) in rows.iter().zip(expected) {
            assert_eq!(row["phase"], json!(phase));
            assert_eq!(row["completed"], json!(completed));
            assert_eq!(row["succeeded"], json!(succeeded));
            assert!(row.contains_key("elapsed_ms"));
        }
    }
}

async fn post_status_fixture() -> (InspectionFixture, Arc<Effects>, Arc<AgentSandboxBackend>) {
    let fixture = inspection_fixture(true).await;
    let effects = Arc::new(Effects::new(&fixture.bound.fixture, BTreeMap::new()));
    let client = AgentSandboxClient::new(
        effects.client.clone(),
        fixture.bound.fixture.client.config.clone(),
        "fixture",
    )
    .unwrap();
    let backend = Arc::new(AgentSandboxBackend::new(client, fixture.transport.clone()));
    backend
        .provision(fixture.bound.request.clone())
        .await
        .unwrap();
    effects.calls.lock().unwrap().clear();
    (fixture, effects, backend)
}

fn arm_after_reply(
    fixture: &InspectionFixture,
    effects: Arc<Effects>,
    gates: BTreeMap<String, Arc<Gate>>,
) {
    *fixture.transport.after_status.lock().unwrap() = Some(Box::new(move || {
        effects.calls.lock().unwrap().clear();
        effects.arm_read_gates(gates.clone());
    }));
}

fn inspecting(
    fixture: &InspectionFixture,
    backend: Arc<AgentSandboxBackend>,
) -> (
    tokio::task::JoinHandle<Result<Option<RuntimeInspection>>>,
    Timing,
) {
    let handle = fixture.bound.runtime.clone();
    let request = fixture.bound.request.clone();
    let home = fixture.home.clone();
    let timing = Timing::default();
    let task = tokio::spawn(
        async move {
            backend
                .inspect_native_runtime(&handle, &request, &home)
                .await
        }
        .with_subscriber(timing.clone()),
    );
    (task, timing)
}

#[tokio::test]
async fn unchanged_post_status_authority_succeeds_with_one_timing_per_phase() {
    let (fixture, effects, backend) = post_status_fixture().await;
    arm_after_reply(&fixture, effects.clone(), BTreeMap::new());
    let before = fixture.bound.fixture.state.lock().unwrap().rows.clone();
    let (task, timing) = inspecting(&fixture, backend);
    let inspected = task.await.unwrap().unwrap().unwrap();
    assert_eq!(inspected.observation.handle, fixture.bound.runtime);
    assert!(inspected.status.unwrap().credential_ready);
    timing.assert_phases(&[
        ("post_status_graph", true, true),
        ("post_status_policy", true, true),
    ]);
    assert_eq!(effects.calls.lock().unwrap().len(), 6);
    assert_eq!(fixture.bound.fixture.state.lock().unwrap().rows, before);
    effects.assert_read_only();
    assert_eq!(fixture.transport.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn post_status_policy_observes_template_changes_while_graph_is_blocked() {
    for remove in [false, true] {
        let (fixture, effects, backend) = post_status_fixture().await;
        let binding = &fixture.bound.claim.request.pool;
        let template = format!("{}/sandboxtemplates/{}", prefix(), binding.template_name);
        let pool = format!("{}/sandboxwarmpools/{}", prefix(), binding.name);
        let graph = Gate::new();
        arm_after_reply(
            &fixture,
            effects.clone(),
            BTreeMap::from([(VOLUME.into(), graph.clone())]),
        );
        let (task, timing) = inspecting(&fixture, backend);
        graph.wait().await;
        assert_eq!(fixture.transport.calls.load(Ordering::SeqCst), 1);
        let calls = effects.calls.lock().unwrap().clone();
        let policy_started_early = calls
            .iter()
            .any(|call| call == &format!("GET {template}") || call == &format!("GET {pool}"));
        {
            let mut state = fixture.bound.fixture.state.lock().unwrap();
            if remove {
                state.rows.remove(&template).unwrap();
            } else {
                state.rows.get_mut(&template).unwrap()["metadata"]["uid"] =
                    json!("replaced-template");
            }
        }
        graph.respond();
        let result = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        assert!(
            result.is_err(),
            "post-response policy change must refuse the captured reply"
        );
        assert!(!policy_started_early);
        timing.assert_phases(&[
            ("post_status_graph", true, true),
            ("post_status_policy", true, false),
        ]);
        effects.assert_read_only();
        assert_eq!(fixture.transport.calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn post_status_graph_refusal_does_not_start_or_wait_for_policy() {
    let (fixture, effects, backend) = post_status_fixture().await;
    let template = format!(
        "{}/sandboxtemplates/{}",
        prefix(),
        fixture.bound.claim.request.pool.template_name
    );
    let graph = Gate::new();
    let stalled_policy = Gate::new();
    arm_after_reply(
        &fixture,
        effects.clone(),
        BTreeMap::from([
            (VOLUME.into(), graph.clone()),
            (template.clone(), stalled_policy),
        ]),
    );
    let (task, timing) = inspecting(&fixture, backend);
    graph.wait().await;
    fixture
        .bound
        .fixture
        .state
        .lock()
        .unwrap()
        .rows
        .get_mut(VOLUME)
        .unwrap()["metadata"]["uid"] = json!("replaced-pvc");
    graph.respond();
    let result = tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(result, Err(AdapterError::Rejected(_))));
    timing.assert_phases(&[("post_status_graph", true, false)]);
    assert!(!effects
        .calls
        .lock()
        .unwrap()
        .contains(&format!("GET {template}")));
    effects.assert_read_only();
    assert_eq!(fixture.transport.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cancelled_post_status_graph_never_starts_policy_or_repeats_status() {
    let (fixture, effects, backend) = post_status_fixture().await;
    let graph = Gate::new();
    arm_after_reply(
        &fixture,
        effects.clone(),
        BTreeMap::from([(VOLUME.into(), graph.clone())]),
    );
    let (task, timing) = inspecting(&fixture, backend);
    graph.wait().await;
    // Aborting drops the future outside WithSubscriber::poll on this test's
    // current-thread runtime, so its Drop event needs the same local subscriber.
    let _capture = tracing::subscriber::set_default(timing.clone());
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    timing.assert_phases(&[("post_status_graph", false, false)]);
    let before = effects.calls.lock().unwrap().clone();
    graph.respond();
    tokio::task::yield_now().await;
    assert_eq!(*effects.calls.lock().unwrap(), before);
    assert!(!before
        .iter()
        .any(|call| call.contains("sandboxtemplates") || call.contains("sandboxwarmpools")));
    effects.assert_read_only();
    assert_eq!(fixture.transport.calls.load(Ordering::SeqCst), 1);
}
