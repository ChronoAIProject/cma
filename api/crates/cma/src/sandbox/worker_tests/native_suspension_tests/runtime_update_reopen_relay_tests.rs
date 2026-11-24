use super::*;
use crate::agent_manager::relay::tests::{acquire_for_store, cluster_with_store, Instance};
use crate::store::sandbox_maintenance::mongo_integration_tests::database;
use crate::store::MongoSandboxStore;
use axum::{
    body::{to_bytes, Body},
    extract::Request,
    http::StatusCode,
    middleware::{from_fn, Next},
    response::Response,
};
use layer2_core::sandbox::store::{SandboxEventStore, WorktreeStore};

#[path = "runtime_update_reopen_release_race.rs"]
mod release_race;
use release_race::ReleaseRace;

type ReopenResponses = Arc<Mutex<Vec<(StatusCode, Option<String>)>>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Caller {
    Enabled,
    FlagOff,
    DifferentRelease,
    BeforeCursorRefusal,
    DurableCurrent,
    PromotedBeforeParking,
    TerminalOwnerRefusal,
    TransientOwnerFailure,
}

pub(crate) async fn exercise_runtime_update_reopen_http() {
    for mode in [
        Caller::FlagOff,
        Caller::DifferentRelease,
        Caller::BeforeCursorRefusal,
        Caller::Enabled,
        Caller::DurableCurrent,
        Caller::PromotedBeforeParking,
        Caller::TerminalOwnerRefusal,
        Caller::TransientOwnerFailure,
    ] {
        exercise(mode).await;
    }
}

async fn exercise(mode: Caller) {
    let (fixture, mut parent, mut agent, now) = at_reopen(true, 0).await;
    let (status, code, submitted, failure_detail) = match mode {
        Caller::TerminalOwnerRefusal => {
            fixture.backend.reject_reopen.store(true, Ordering::SeqCst);
            (
                StatusCode::CONFLICT,
                Some("owner_reopen_rejected"),
                &[][..],
                Some("runtime rejected the lifecycle operation"),
            )
        }
        Caller::TransientOwnerFailure => {
            fixture
                .backend
                .fail_reopen_transiently
                .store(true, Ordering::SeqCst);
            (StatusCode::SERVICE_UNAVAILABLE, None, &[][..], None)
        }
        _ => {
            *fixture.backend.reopen_output.cursors.lock().unwrap() = vec![2];
            (
                StatusCode::CONFLICT,
                Some("owner_reopen_output_discontinuity"),
                &[2][..],
                matches!(mode, Caller::Enabled | Caller::DurableCurrent)
                    .then_some("retained conversation output is discontinuous"),
            )
        }
    };

    let (mongo, database_name, store) = database().await;
    let race = if matches!(mode, Caller::DurableCurrent | Caller::PromotedBeforeParking) {
        Some(Arc::new(ReleaseRace::new(&mongo, &database_name).await))
    } else {
        None
    };
    let store = match &race {
        Some(race) => crate::store::open_pointer_fixture_sandbox_store(
            &mongo.database(&database_name),
            TARGET.into(),
            race.source.clone(),
        )
        .await
        .unwrap(),
        None => store,
    };
    let store = Arc::new(
        store
            .enable_agent_ownership("reopen-http-test".into())
            .await
            .unwrap(),
    );
    parent.revision = 1;
    // The in-memory fixture advances its clock through each restore phase.
    // Mongo gates acquisition on its own clock, so seed a due work row.
    parent.work.ready_at = layer2_core::now_rfc3339();
    store.create_sandbox(parent.clone()).await.unwrap();
    let mut worktree = fixture
        .harness
        .store
        .get_worktree(&agent.worktree_id)
        .await
        .unwrap()
        .unwrap();
    worktree.revision = 1;
    store.create_worktree(worktree).await.unwrap();
    agent.revision = 1;
    agent.conversation.last_seq = 0;
    store.create_agent(agent.clone()).await.unwrap();

    let responses = Arc::new(Mutex::new(Vec::new()));
    let owners = store.agent_ownership.as_ref().unwrap();
    let instances = cluster_with_store(2, owners.clone(), |router| {
        let responses = responses.clone();
        let race = race.clone();
        router.layer(from_fn(move |request: Request, next: Next| {
            observe_reopen_response(request, next, responses.clone(), mode, race.clone())
        }))
    })
    .await;
    let (owner, _) = acquire_for_store(owners.as_ref(), &instances, &agent.id).await;
    let caller = 1 - owner;
    let caller_image = if mode == Caller::DifferentRelease {
        runtime_image()
    } else {
        TARGET.into()
    };
    let _owner = worker(
        &fixture,
        &instances[owner],
        store.clone(),
        TARGET,
        true,
        None,
    );
    let worker = worker(
        &fixture,
        &instances[caller],
        store.clone(),
        &caller_image,
        !matches!(mode, Caller::FlagOff | Caller::BeforeCursorRefusal),
        race.as_deref(),
    );

    let result = worker
        .run_one(&parent.id, now + Duration::from_secs(6))
        .await;
    assert_eq!(
        result,
        Ok(true),
        "{mode:?} responses={:?}",
        responses.lock().unwrap()
    );
    let after = store.get_sandbox(&parent.id).await.unwrap().unwrap();
    assert_eq!(
        *responses.lock().unwrap(),
        [(status, code.map(str::to_owned))],
        "{mode:?}"
    );
    assert_eq!(
        *fixture.backend.reopen_output.submitted.lock().unwrap(),
        submitted
    );
    assert!(fixture
        .backend
        .reopen_output
        .acknowledged
        .lock()
        .unwrap()
        .is_empty());
    assert!(store
        .agent_message_checkpoint(&agent.id)
        .await
        .unwrap()
        .is_none());
    assert_eq!(store.get_agent(&agent.id).await.unwrap().unwrap(), agent);
    assert_eq!(after.lifecycle.operation, parent.lifecycle.operation);
    assert_eq!(after.lifecycle.home, parent.lifecycle.home);
    assert_eq!(after.runtime, parent.runtime);
    assert!(after.work.lease.is_none());

    let failure = after.runtime_update_request().unwrap().failure.as_ref();
    let parked = failure_detail.is_some();
    if let Some(detail) = failure_detail {
        assert_eq!(after.work.step, SandboxWorkStep::None);
        assert_eq!(failure.unwrap().detail, detail);
        assert_eq!(failure.unwrap().class, "terminal_validation");
    } else {
        assert_eq!(
            after.work.step,
            SandboxWorkStep::RunLifecycleOperation,
            "{mode:?}"
        );
        assert!(failure.is_none(), "{mode:?}");
        if mode == Caller::TransientOwnerFailure {
            assert!(after.work.ready_at > parent.work.ready_at);
            assert!(after.work.attempts > parent.work.attempts);
        }
    }
    if let Some(race) = race {
        use layer3_adapter::ReleasedRuntimeSource;
        let expected = if parked { TARGET } else { release_race::NEXT };
        assert_eq!(race.source.snapshot().agent, expected);
        let events = store.sandbox_events(&parent.id, 0, 100).await.unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    &event.kind,
                    layer2_core::sandbox::SandboxEventKind::WorkStepFailed { failure, .. }
                    if failure.class == "terminal_validation"
                ))
                .count(),
            usize::from(parked),
        );
    }
    drop(instances);
    mongo.database(&database_name).drop().await.unwrap();
}

fn worker(
    fixture: &Fixture,
    instance: &Instance,
    store: Arc<MongoSandboxStore>,
    released: &str,
    enabled: bool,
    race: Option<&ReleaseRace>,
) -> Arc<SandboxWorker> {
    let registry = Arc::new(LiveAgentRegistry::default());
    registry.install_ownership(instance.coordinator.clone());
    let mut config = SandboxWorkerConfig::new(
        &instance.coordinator.instance_id,
        "test",
        released,
        fixture.harness.policy.clone(),
    );
    config.reopen_output_refusal_enabled = enabled;
    if let Some(race) = race {
        config = config.with_released_runtime(race.source.clone());
    }
    let worker = SandboxWorker::new(
        SandboxWorkerDependencies {
            store: store.clone(),
            adapter: fixture.adapter(TARGET),
            credentials: fixture.harness.credentials.clone(),
            workspaces: Arc::new(WorkspaceRecorder {
                actions: fixture.harness.actions.clone(),
            }),
            agents: Arc::new(AgentStopRecorder {
                actions: fixture.harness.actions.clone(),
            }),
            registry: registry.clone(),
            updates: DurableAgentUpdateRecorder::new(store.clone()),
            operations: AgentOperationService::new(store.clone(), registry),
        },
        config,
    );
    instance.relay.install_reopen(worker.clone(), store);
    worker
}

async fn observe_reopen_response(
    request: Request,
    next: Next,
    responses: ReopenResponses,
    mode: Caller,
    race: Option<Arc<ReleaseRace>>,
) -> Response {
    let reopen = request.uri().path().ends_with("/reopen");
    let response = next.run(request).await;
    if !reopen {
        return response;
    }
    if response.status() != StatusCode::CONFLICT {
        responses.lock().unwrap().push((response.status(), None));
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let bytes = to_bytes(body, 4096).await.unwrap();
    let problem: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let code = problem["code"].as_str().unwrap();
    responses
        .lock()
        .unwrap()
        .push((parts.status, Some(code.to_owned())));
    if mode == Caller::PromotedBeforeParking {
        assert_eq!(code, "owner_reopen_output_discontinuity");
        race.unwrap().promote_after_refusal().await;
    }
    // The #3265 caller recognizes precisely this one conflict code. Every
    // other 409 yields the same unavailable result as a 503.
    if mode == Caller::BeforeCursorRefusal && code != "owner_reopen_rejected" {
        parts.status = StatusCode::SERVICE_UNAVAILABLE;
    }
    Response::from_parts(parts, Body::from(bytes))
}
