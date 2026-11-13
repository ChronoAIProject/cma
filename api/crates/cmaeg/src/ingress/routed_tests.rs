#[path = "routed_custom_tests.rs"]
mod custom_tests;
#[path = "routed_quote_tests.rs"]
mod quote_tests;
use super::*;
use crate::{
    channels::resolver::ChannelResolver,
    ingress::{launch::TurnLauncher, routing_plan::fixture, routing_store::test_server::Server},
    turns::routed::scheduler::{Disposition, ScheduleConfig, Scheduler, TargetExecutor, WorkItem},
};
use cmaeg_core::{activity::SourceKind, seal::event_ref::activity_digest};
use futures_util::future::BoxFuture;
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Legacy(AtomicUsize);
impl TurnLauncher for Legacy {
    fn launch(&self, _: LaunchRequest) -> BoxFuture<'_, LaunchOutcome> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            LaunchOutcome::Accepted {
                response_id: "legacy-response".into(),
            }
        })
    }
}
struct Executor;
impl TargetExecutor for Executor {
    fn execute<'a>(&'a self, _: &'a WorkItem) -> BoxFuture<'a, Disposition> {
        Box::pin(async { Disposition::Settled })
    }
}
struct Setup {
    server: Server,
    state: IngressState,
    legacy: Arc<Legacy>,
    _scheduler: Scheduler,
}
impl Setup {
    async fn new(request: &LaunchRequest, selected: bool) -> Self {
        let server = Server::start().await;
        let keys = Arc::new(fixture::keys());
        let (scheduler, handle) = Scheduler::new(
            Arc::new(server.store()),
            Arc::new(Executor),
            ScheduleConfig {
                queue_capacity: 1,
                ..Default::default()
            },
        )
        .unwrap();
        let records = if selected {
            BTreeSet::from([request.channel.definition.record_id.to_string()])
        } else {
            BTreeSet::new()
        };
        let mut routed = RoutedIngress::new(
            server.store(),
            handle,
            keys.clone(),
            records,
            BTreeSet::from(["cma_codex".into()]),
            BTreeSet::from(["cma_codex".into(), "nyxbot".into()]),
        );
        routed.clock = Arc::new(|| fixture::NOW);
        let legacy = Arc::new(Legacy(AtomicUsize::new(0)));
        let state = IngressState::new(
            ChannelResolver::new(
                Arc::new(server.records.clone()),
                keys.clone(),
                Default::default(),
            ),
            keys,
            Default::default(),
            legacy.clone(),
        )
        .unwrap()
        .with_routed(Some(Arc::new(routed)));
        Self {
            server,
            state,
            legacy,
            _scheduler: scheduler,
        }
    }
    async fn launch(&self, request: LaunchRequest, bytes: &[u8], signed: bool) -> LaunchOutcome {
        let source = self.state.capture_routed(&request, bytes, signed);
        let permit = self.state.inflight.clone().try_acquire_owned().unwrap();
        self.state.launch_verified(request, source, permit).await
    }

    async fn launch_github(&self, request: LaunchRequest, bytes: &[u8]) -> LaunchOutcome {
        self.state
            .launch_github(
                request,
                bytes.to_vec(),
                super::github::GithubAuthorityFence {
                    owner: "user-1".into(),
                    integration_id: "github-pilot".into(),
                    integration_version: 1,
                    binding_id: "binding-fixture".into(),
                    binding_version: 1,
                },
            )
            .await
    }
}
fn request() -> LaunchRequest {
    fixture::request(fixture::targets, fixture::message("original message"))
}
fn admitted(value: LaunchOutcome) -> (String, cmaeg_core::ids::ConversationId) {
    match value {
        LaunchOutcome::Admitted {
            admission_id,
            conversation_id,
            ..
        } => (admission_id, conversation_id),
        other => panic!("expected durable admission: {other:?}"),
    }
}

fn sync_request(waiter: Option<&str>) -> LaunchRequest {
    let mut request = fixture::request(fixture::targets, fixture::custom(None));
    fixture::rewrite_ref(&mut request, |event| {
        event.reply = waiter.map_or(cmaeg_core::seal::ReplyAddress::None, |id| {
            cmaeg_core::seal::ReplyAddress::Sync { waiter: id.into() }
        });
    });
    request
}

fn github_request() -> LaunchRequest {
    let mut request = fixture::request(
        |value| {
            value["sources"].as_array_mut().unwrap().push(json!({
                "type": "github",
                "app_ref": "github-pilot",
                "installation_ids": [7],
                "repository_ids": [42],
                "events": ["issues"],
                "actions": ["opened"]
            }));
        },
        fixture::custom(None),
    );
    request.activity.source = SourceKind::Github {
        app_ref: "github-pilot".into(),
        installation_id: 7,
        repository_id: 42,
    };
    let digest = activity_digest(&request.activity);
    fixture::rewrite_ref(&mut request, |event| {
        event.activity_sha256 = Some(digest);
    });
    request
}

#[tokio::test]
async fn github_event_refuses_when_the_guarded_root_is_unavailable() {
    let request = github_request();
    let setup = Setup::new(&request, true).await;

    let outcome = setup
        .launch_github(request, br#"{"action":"opened"}"#)
        .await;
    assert!(matches!(
        outcome,
        LaunchOutcome::Refused(problem)
            if problem.detail.as_deref() == Some("github_channel_removed_before_admission")
    ));
    assert_eq!(setup.legacy.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn restored_admission_attaches_only_the_original_primary_waiter() {
    for original_waiter in [None, Some("original-waiter")] {
        let original = sync_request(original_waiter);
        let setup = Setup::new(&original, true).await;
        let candidate = Candidate::prepare(
            &original,
            &fixture::routed(&original),
            b"wire",
            &fixture::keys(),
            fixture::NOW,
        )
        .unwrap();
        setup.server.store().admit(&candidate).await.unwrap();
        let writes = setup.server.memory.writes.load(Ordering::SeqCst);
        if original_waiter.is_some() {
            assert!(matches!(
                setup.launch(original, b"wire", true).await,
                LaunchOutcome::Admitted {
                    sync_waiter_attached: true,
                    ..
                }
            ));
        }
        assert!(matches!(
            setup
                .launch(sync_request(Some("retry-waiter")), b"wire", true)
                .await,
            LaunchOutcome::Admitted {
                sync_waiter_attached: false,
                ..
            }
        ));
        assert!(matches!(
            setup.launch(sync_request(None), b"wire", true).await,
            LaunchOutcome::Admitted {
                sync_waiter_attached: false,
                ..
            }
        ));
        assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), writes);
        assert_eq!(setup.legacy.0.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn concurrent_stream_candidates_keep_one_attached_waiter() {
    let left = sync_request(Some("left-waiter"));
    let right = sync_request(Some("right-waiter"));
    let setup = Setup::new(&left, true).await;
    let left_candidate = Candidate::prepare(
        &left,
        &fixture::routed(&left),
        b"wire",
        &fixture::keys(),
        fixture::NOW,
    )
    .unwrap();
    let right_candidate = Candidate::prepare(
        &right,
        &fixture::routed(&right),
        b"wire",
        &fixture::keys(),
        fixture::NOW,
    )
    .unwrap();
    let store = setup.server.store();
    let (left_plan, right_plan) =
        tokio::join!(store.admit(&left_candidate), store.admit(&right_candidate));
    assert_eq!(
        left_plan.unwrap().targets[0].launch.event.reply,
        right_plan.unwrap().targets[0].launch.event.reply
    );
    let (left, right) = tokio::join!(
        setup.launch(left, b"wire", true),
        setup.launch(right, b"wire", true)
    );
    let attached = [left, right]
        .into_iter()
        .filter(|outcome| {
            matches!(
                outcome,
                LaunchOutcome::Admitted {
                    sync_waiter_attached: true,
                    ..
                }
            )
        })
        .count();
    assert_eq!(attached, 1);
    assert_eq!(setup.legacy.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn unselected_existing_channels_do_not_read_store_or_retain_source_bytes() {
    let request = request();
    let setup = Setup::new(&request, false).await;
    setup
        .server
        .memory
        .unavailable_reads
        .store(1, Ordering::SeqCst);
    assert!(setup
        .state
        .capture_routed(&request, b"wire", true)
        .is_none());
    assert!(matches!(
        setup.launch(request, b"wire", true).await,
        LaunchOutcome::Accepted { .. }
    ));
    assert_eq!(setup.legacy.0.load(Ordering::SeqCst), 1);
    assert_eq!(
        setup.server.memory.unavailable_reads.load(Ordering::SeqCst),
        1
    );
    assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn durable_acceptance_keeps_raw_bytes_and_original_primary_even_when_hint_queue_is_full() {
    let request = request();
    let expected = fixture::routed(&request);
    let setup = Setup::new(&request, true).await;
    let result = admitted(
        setup
            .launch(request, b"original exact wire bytes", true)
            .await,
    );
    assert_eq!(result.1, expected.targets[0].event.conversation_id);
    assert_eq!(setup.legacy.0.load(Ordering::SeqCst), 0);
    let page = setup
        .server
        .store()
        .recover_all_page(None, 32)
        .await
        .unwrap();
    assert_eq!(page.plans.len(), 1);
    assert_eq!(page.plans[0].admission_id, result.0);
    assert_eq!(page.plans[0].source_body, b"original exact wire bytes");
    assert_eq!(page.plans[0].targets.len(), 3);
    assert_eq!(
        setup.state.inflight.available_permits(),
        setup.state.config.max_inflight
    );
}

#[tokio::test]
async fn retry_restores_winning_snapshot_after_policy_removal_and_changed_wire_body_conflicts() {
    let setup = Setup::new(&request(), true).await;
    let first = admitted(setup.launch(request(), b"wire", true).await);
    let writes = setup.server.memory.writes.load(Ordering::SeqCst);
    let mut changed = request();
    Arc::get_mut(&mut changed.channel)
        .unwrap()
        .definition
        .routing = None;
    assert_eq!(admitted(setup.launch(changed, b"wire", true).await), first);
    assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), writes);
    let outcome = setup.launch(request(), b"different wire", true).await;
    assert!(matches!(
        outcome,
        LaunchOutcome::Refused(Problem {
            code: ProblemCode::Conflict,
            status: 409,
            ..
        })
    ));
    assert_eq!(setup.legacy.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn partial_commit_is_never_acknowledged_and_same_source_retries_without_legacy_launch() {
    let request = request();
    let setup = Setup::new(&request, true).await;
    let candidate = Candidate::prepare(
        &request,
        &fixture::routed(&request),
        b"wire",
        &fixture::keys(),
        fixture::NOW,
    )
    .unwrap();
    setup
        .server
        .fault(candidate.admission_id().into(), false, false);
    assert!(matches!(
        setup.launch(request, b"wire", true).await,
        LaunchOutcome::Refused(Problem { status: 503, .. })
    ));
    assert!(setup
        .server
        .store()
        .load(candidate.identity())
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        admitted(setup.launch(self::request(), b"wire", true).await).0,
        candidate.admission_id()
    );
    assert_eq!(setup.legacy.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn concurrent_duplicate_requests_commit_one_snapshot_and_do_not_launch_legacy() {
    let setup = Setup::new(&request(), true).await;
    let (one, two) = tokio::join!(
        setup.launch(request(), b"wire", true),
        setup.launch(request(), b"wire", true)
    );
    assert_eq!(admitted(one), admitted(two));
    assert_eq!(
        setup
            .server
            .store()
            .recover_all_page(None, 32)
            .await
            .unwrap()
            .plans
            .len(),
        1
    );
    assert_eq!(setup.legacy.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn unavailable_store_or_missing_verified_bytes_cannot_fall_back_to_live_execution() {
    let setup = Setup::new(&request(), true).await;
    setup
        .server
        .memory
        .unavailable_reads
        .store(1, Ordering::SeqCst);
    assert!(matches!(
        setup.launch(request(), b"wire", true).await,
        LaunchOutcome::Refused(Problem { status: 503, .. })
    ));
    let permit = setup.state.inflight.clone().try_acquire_owned().unwrap();
    assert!(matches!(
        setup.state.launch_verified(request(), None, permit).await,
        LaunchOutcome::Refused(Problem { status: 503, .. })
    ));
    assert_eq!(setup.legacy.0.load(Ordering::SeqCst), 0);
    assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn selected_but_unrouted_channel_retains_legacy_launch_until_a_plan_is_committed() {
    let mut request = request();
    Arc::get_mut(&mut request.channel)
        .unwrap()
        .definition
        .routing = None;
    let setup = Setup::new(&request, true).await;
    assert!(matches!(
        setup.launch(request, b"wire", true).await,
        LaunchOutcome::Accepted { .. }
    ));
    assert_eq!(setup.legacy.0.load(Ordering::SeqCst), 1);
    assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn unproved_provider_replay_is_refused_before_durable_admission() {
    let mut setup = Setup::new(&request(), true).await;
    Arc::get_mut(setup.state.routed.as_mut().unwrap())
        .unwrap()
        .replay_providers
        .clear();
    assert!(matches!(
        setup.launch(request(), b"wire", true).await,
        LaunchOutcome::Refused(Problem { status: 503, .. })
    ));
    assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), 0);
    assert_eq!(setup.legacy.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn unsupported_delivery_never_commits_or_launches_but_retries_keep_original_plan() {
    let mut sync = request();
    fixture::rewrite_ref(&mut sync, |event| {
        event.reply = cmaeg_core::seal::ReplyAddress::Sync {
            waiter: "ephemeral-waiter".into(),
        };
    });
    let setup = Setup::new(&sync, true).await;
    assert!(matches!(
        setup.launch(sync, b"wire", true).await,
        LaunchOutcome::Refused(Problem { status: 422, .. })
    ));
    assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), 0);
    assert_eq!(setup.legacy.0.load(Ordering::SeqCst), 0);

    let first = admitted(setup.launch(request(), b"wire", true).await);
    let writes = setup.server.memory.writes.load(Ordering::SeqCst);
    let mut changed = request();
    Arc::get_mut(&mut changed.channel)
        .unwrap()
        .definition
        .routing
        .as_mut()
        .unwrap()
        .consumers[0]
        .reply
        .destination = cmaeg_core::channel::ReplyChoice::Default;
    assert_eq!(admitted(setup.launch(changed, b"wire", true).await), first);
    assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), writes);
    assert_eq!(setup.legacy.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn custom_sync_admission_requires_the_explicit_shared_stream_composition() {
    let request = || {
        let mut request = fixture::request(
            |value| {
                fixture::targets(value);
                value["reply"]["sinks"] = serde_json::json!([{"type":"sync"}]);
            },
            fixture::custom(None),
        );
        fixture::rewrite_ref(&mut request, |event| {
            event.reply = cmaeg_core::seal::ReplyAddress::Sync {
                waiter: "shared-custom-waiter".into(),
            };
        });
        request
    };
    let mut setup = Setup::new(&request(), true).await;
    assert!(matches!(
        setup.launch(request(), b"custom-event", true).await,
        LaunchOutcome::Refused(Problem { status: 422, .. })
    ));
    assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), 0);
    Arc::get_mut(setup.state.routed.as_mut().unwrap())
        .unwrap()
        .sync_delivery_enabled = true;
    admitted(setup.launch(request(), b"custom-event", true).await);
    assert_eq!(
        setup
            .server
            .store()
            .recover_all_page(None, 32)
            .await
            .unwrap()
            .plans
            .len(),
        1
    );
    assert_eq!(setup.legacy.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn pausing_admission_keeps_committed_retries_and_refuses_new_work_before_writes() {
    let mut setup = Setup::new(&request(), true).await;
    let first = admitted(setup.launch(request(), b"wire", true).await);
    Arc::get_mut(setup.state.routed.as_mut().unwrap())
        .unwrap()
        .admission_enabled = false;
    let writes = setup.server.memory.writes.load(Ordering::SeqCst);
    assert_eq!(
        admitted(setup.launch(request(), b"wire", true).await),
        first
    );
    let mut another = request();
    another.activity.event_id = fixture::event_id("new-event");
    another.idempotency_key =
        cmaeg_core::ids::IdempotencyKey::from_event(&another.activity.event_id);
    let activity = another.activity.clone();
    fixture::rewrite_ref(&mut another, |event| {
        event.event_id = activity.event_id.clone();
        event.activity_sha256 = Some(cmaeg_core::seal::event_ref::activity_digest(&activity));
    });
    assert!(matches!(
        setup.launch(another, b"new-wire", true).await,
        LaunchOutcome::Refused(Problem { status: 503, .. })
    ));
    assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), writes);
    assert_eq!(setup.legacy.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn stopped_scheduler_refuses_new_admission_before_writing() {
    let request = request();
    let setup = Setup::new(&request, true).await;
    drop(setup._scheduler);
    let captured = setup.state.capture_routed(&request, b"wire", true);
    let permit = setup.state.inflight.clone().try_acquire_owned().unwrap();
    assert!(matches!(
        setup.state.launch_verified(request, captured, permit).await,
        LaunchOutcome::Refused(Problem { status: 503, .. })
    ));
    assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), 0);
    assert_eq!(setup.legacy.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn oversized_selected_body_is_not_copied_or_admitted() {
    let request = request();
    let setup = Setup::new(&request, true).await;
    let bytes = vec![b'x'; MAX_SOURCE_BYTES + 1];
    let captured = setup.state.capture_routed(&request, &bytes, true).unwrap();
    assert!(captured.body.is_empty());
    assert!(captured.oversized);
    let permit = setup.state.inflight.clone().try_acquire_owned().unwrap();
    assert!(matches!(
        setup
            .state
            .launch_verified(request, Some(captured), permit)
            .await,
        LaunchOutcome::Refused(Problem { status: 413, .. })
    ));
    assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), 0);
    assert_eq!(setup.legacy.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn unsigned_binary_custom_target_attributes_cannot_select_consumers() {
    let request = fixture::request(fixture::targets, fixture::custom(Some(fixture::REVIEWER)));
    let setup = Setup::new(&request, true).await;
    assert!(matches!(
        setup.launch(request, b"binary data", false).await,
        LaunchOutcome::Refused(_)
    ));
    assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), 0);
    assert_eq!(setup.legacy.0.load(Ordering::SeqCst), 0);
}
