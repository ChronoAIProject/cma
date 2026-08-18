//! What a session owner is promised, expressed as tests.
//!
//! The promise is short: a session you did not stop keeps working, a session
//! you stopped stays readable and can be picked up again, and neither of those
//! depends on any process staying alive. Everything here is a way of taking
//! that promise away — restarting the control plane, losing a sandbox, asking a
//! provider that will not answer — and checking it survives.
//!
//! "Restart" is expressible because a service shares nothing with its successor
//! except the store, so a second `service()` over the same store is a new
//! process in every way that matters here.

use std::sync::Arc;

use crate::service::Deployment;
use crate::session::{SessionStatus, StopReason};
use crate::store::SessionStore;
use crate::test_support::{await_settled, service, spec, user, FakeAdapter, FakeStore};

#[tokio::test]
async fn a_running_session_survives_the_control_plane_restarting() {
    // The headline promise. Nothing the owner did ended this session, so the
    // deployment that replaced the process serving it must not either.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");

    let first = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = first.create_session(&alice, spec()).await.expect("created");
    await_settled(&first, &alice, &created.id).await;
    let original = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present")
        .runtime_id
        .expect("has a runtime");

    // The process goes. The sandbox does not.
    drop(first);
    let restarted = service(Arc::clone(&store), Arc::clone(&adapter));
    let recovered = restarted.reconcile().await;

    assert_eq!(
        recovered.readopted_sessions, 1,
        "the running session must be taken back over, not started again"
    );
    assert_eq!(recovered.reprovisioned_sessions, 0);
    assert_eq!(
        store
            .get(&created.id)
            .await
            .expect("readable")
            .expect("present")
            .runtime_id,
        Some(original),
        "taking a session back over must not change the sandbox it is using"
    );
    assert!(
        adapter.terminated.lock().await.is_empty(),
        "nothing about a restart justifies tearing a live session down"
    );
}

#[tokio::test]
async fn a_session_whose_sandbox_vanished_is_given_another() {
    // A node is replaced and the sandbox goes with it. The owner did not ask
    // for that and should not have to notice it.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");

    let first = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = first.create_session(&alice, spec()).await.expect("created");
    await_settled(&first, &alice, &created.id).await;
    let lost = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present")
        .runtime_id
        .expect("has a runtime");

    drop(first);
    adapter.lose(&lost).await;

    let restarted = service(Arc::clone(&store), Arc::clone(&adapter));
    let recovered = restarted.reconcile().await;
    assert_eq!(recovered.reprovisioned_sessions, 1);

    let settled = await_settled(&restarted, &alice, &created.id).await;
    assert_eq!(
        settled.status,
        SessionStatus::Ready,
        "a session that lost its sandbox must end up usable again, not failed"
    );
    let now = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present")
        .runtime_id
        .expect("has a runtime");
    assert_ne!(
        now, lost,
        "it must be a new sandbox, and be recorded as one"
    );
}

#[tokio::test]
async fn recovery_keeps_the_session_the_owner_configured() {
    // Replacing the sandbox is invisible only if what comes back is the same
    // session. A recovered session running a different model would be a
    // different session wearing the same identifier.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");

    // The model is pinned where it now lives: on what the session is started
    // from, rather than on the request. The guarantee under test is unchanged —
    // whatever the session was started with must survive its sandbox.
    let pinned =
        crate::test_support::environments_defaulting_to(crate::environment::AgentConfiguration {
            kernel: crate::test_support::kernel(),
            model: Some("a-specific-model".to_string()),
            ..Default::default()
        });
    let chosen = spec();

    let first = crate::test_support::service_with_environments(
        Arc::clone(&store),
        Arc::clone(&adapter),
        Arc::clone(&pinned),
    );
    let created = first.create_session(&alice, chosen).await.expect("created");
    await_settled(&first, &alice, &created.id).await;
    let lost = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present")
        .runtime_id
        .expect("has a runtime");

    drop(first);
    adapter.lose(&lost).await;
    let restarted = crate::test_support::service_with_environments(
        Arc::clone(&store),
        Arc::clone(&adapter),
        Arc::clone(&pinned),
    );
    restarted.reconcile().await;
    await_settled(&restarted, &alice, &created.id).await;

    let record = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present");
    assert_eq!(
        record.session.options.model.as_deref(),
        Some("a-specific-model"),
        "the options the owner chose must outlive the sandbox they were chosen for"
    );
    assert_eq!(record.session.id, created.id, "and so must the identity");
}

#[tokio::test]
async fn a_runtime_another_control_plane_created_is_left_alone() {
    // A sandbox provider is shared: one key sees everything provisioned against
    // it. A second control plane — staging, mid-rollout, or one being verified
    // against production infrastructure — would otherwise read every session it
    // has never heard of as an orphan and tear it down on its first sweep.
    let store = Arc::new(FakeStore::default());
    let theirs = Arc::new(FakeAdapter {
        created_by: Some("some-other-control-plane".to_string()),
        ..FakeAdapter::default()
    });
    theirs
        .running
        .lock()
        .await
        .insert("their-runtime".to_string(), Some("sesn_theirs".to_string()));

    let ours = crate::test_support::service_as(
        Arc::clone(&store),
        Arc::clone(&theirs),
        Deployment {
            identity: Some("ours".to_string()),
            adopts_untagged: true,
            ..Deployment::default()
        },
    );
    let recovered = ours.reconcile().await;

    assert_eq!(
        recovered.orphaned_runtimes, 0,
        "a runtime belonging to another control plane must never be reclaimed"
    );
    assert!(
        theirs.terminated.lock().await.is_empty(),
        "and must certainly not be torn down"
    );
}

#[tokio::test]
async fn a_secondary_control_plane_leaves_unnamed_runtimes_alone() {
    // The runtimes that predate deployment names are the first deployment's.
    // A second one sharing the provider has no way to tell whether they should
    // exist, so it must not decide.
    let store = Arc::new(FakeStore::default());
    let provider = Arc::new(FakeAdapter::default());
    provider
        .running
        .lock()
        .await
        .insert("older-runtime".to_string(), Some("sesn_older".to_string()));

    let secondary = crate::test_support::service_as(
        Arc::clone(&store),
        Arc::clone(&provider),
        Deployment {
            identity: Some("verification".to_string()),
            adopts_untagged: false,
            ..Deployment::default()
        },
    );
    let recovered = secondary.reconcile().await;

    assert_eq!(recovered.orphaned_runtimes, 0);
    assert!(provider.terminated.lock().await.is_empty());
}

#[tokio::test]
async fn the_deployment_that_owns_the_provider_still_reclaims_orphans() {
    // The other half of the rule: scoping reclamation must not stop the primary
    // deployment cleaning up after itself, or a sandbox created moments before
    // a crash would never be released.
    let store = Arc::new(FakeStore::default());
    let provider = Arc::new(FakeAdapter::default());
    provider
        .running
        .lock()
        .await
        .insert("stray".to_string(), Some("sesn_nobody".to_string()));

    let primary = crate::test_support::service_as(
        Arc::clone(&store),
        Arc::clone(&provider),
        Deployment {
            identity: Some("ours".to_string()),
            adopts_untagged: true,
            ..Deployment::default()
        },
    );
    let recovered = primary.reconcile().await;

    assert_eq!(
        recovered.orphaned_runtimes, 1,
        "a runtime no session refers to is still reclaimed"
    );
}

#[tokio::test]
async fn a_provider_that_cannot_be_asked_changes_nothing() {
    // The dangerous reading of silence. An enumeration that fails says nothing
    // about what is running, and treating it as "nothing is running" would tear
    // down or replace every live session at once.
    let store = Arc::new(FakeStore::default());
    let alice = user("alice");

    let first = service(Arc::clone(&store), Arc::new(FakeAdapter::default()));
    let created = first.create_session(&alice, spec()).await.expect("created");
    await_settled(&first, &alice, &created.id).await;
    drop(first);

    let mute = Arc::new(FakeAdapter {
        refuse_list: true,
        ..FakeAdapter::default()
    });
    // The sandbox exists as far as attaching is concerned; only the question
    // "what are you running" goes unanswered.
    let handle = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present")
        .runtime_id
        .expect("has a runtime");
    mute.running
        .lock()
        .await
        .insert(handle, Some(created.id.0.clone()));

    let restarted = service(Arc::clone(&store), Arc::clone(&mute));
    let recovered = restarted.reconcile().await;

    assert_eq!(
        recovered.orphaned_runtimes, 0,
        "a provider that cannot be asked must not have its runtimes reclaimed"
    );
    assert_eq!(
        recovered.reprovisioned_sessions, 0,
        "nor its sessions replaced"
    );
    assert_eq!(
        recovered.readopted_sessions, 1,
        "attaching answers what enumeration could not, so the session comes back"
    );
}

#[tokio::test]
async fn a_session_being_started_is_left_alone() {
    // Bring-up is detached and slow, so a session in the middle of starting
    // looks exactly like one whose start-up died. Acting on that would hand one
    // session two sandboxes.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");
    let svc = service(Arc::clone(&store), Arc::clone(&adapter));

    let created = svc.create_session(&alice, spec()).await.expect("created");
    // Deliberately without waiting: the claim is held from before the record is
    // written until bring-up ends.
    svc.begin_start(&created.id).await;
    let recovered = svc.reconcile().await;

    assert_eq!(
        recovered.reprovisioned_sessions, 0,
        "a session already being started must not be started again"
    );
    svc.finish_start(&created.id).await;
}

#[tokio::test]
async fn stopping_keeps_the_session_and_releases_the_sandbox() {
    // Stopping is the reversible half of ending. The point is that the bill
    // stops without the work being lost.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");
    let svc = service(Arc::clone(&store), Arc::clone(&adapter));

    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;
    svc.stop_session(&alice, &created.id)
        .await
        .expect("a running session can be stopped");

    let record = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present");
    assert_eq!(record.session.status, SessionStatus::Idle);
    assert_eq!(record.session.stop_reason, Some(StopReason::UserTerminated));
    assert!(
        !record.session.status.is_terminal(),
        "stopping must leave the session revivable"
    );
    assert_eq!(
        adapter.running_count().await,
        0,
        "the sandbox must actually be released"
    );

    let listed = svc.list_sessions(&alice).await;
    assert!(
        listed.iter().any(|s| s.id == created.id),
        "a stopped session must still be listed: stopping is not deleting"
    );
}

#[tokio::test]
async fn a_stopped_session_is_not_given_a_sandbox_behind_the_owners_back() {
    // The controller starts anything the record says should be running. A
    // stopped session says the opposite, and a controller that could not tell
    // the difference would undo every stop within a sweep.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");
    let svc = service(Arc::clone(&store), Arc::clone(&adapter));

    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;
    svc.stop_session(&alice, &created.id)
        .await
        .expect("stopped");

    let recovered = svc.reconcile().await;
    assert_eq!(recovered.reprovisioned_sessions, 0);
    assert_eq!(recovered.readopted_sessions, 0);
    assert_eq!(
        adapter.running_count().await,
        0,
        "nothing may be running for a session its owner stopped"
    );
    assert_eq!(
        store
            .get(&created.id)
            .await
            .expect("readable")
            .expect("present")
            .session
            .status,
        SessionStatus::Idle
    );
}

#[tokio::test]
async fn reviving_returns_the_same_session_to_use() {
    // What the owner gets back must be the session they stopped — same
    // identifier, same history — or "revive" would mean "start something else".
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");
    let svc = service(Arc::clone(&store), Arc::clone(&adapter));

    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;
    let before = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present")
        .session
        .last_seq;
    svc.stop_session(&alice, &created.id)
        .await
        .expect("stopped");

    let revived = svc
        .revive_session(&alice, &created.id)
        .await
        .expect("a stopped session can be revived");
    assert_eq!(
        revived.id, created.id,
        "reviving must not mint a new session"
    );

    let settled = await_settled(&svc, &alice, &created.id).await;
    assert_eq!(settled.status, SessionStatus::Ready);
    assert!(
        settled.last_seq > before,
        "the history must continue rather than restart"
    );
    assert_eq!(adapter.running_count().await, 1);
}

#[tokio::test]
async fn a_deleted_session_cannot_be_revived() {
    // Delete is the one action with no way back, so it has to actually refuse.
    let store = Arc::new(FakeStore::default());
    let alice = user("alice");
    let svc = service(Arc::clone(&store), Arc::new(FakeAdapter::default()));

    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;
    svc.delete_session(&alice, &created.id)
        .await
        .expect("deletable");

    let revived = svc.revive_session(&alice, &created.id).await;
    assert!(
        revived.is_err(),
        "a session that no longer exists must not come back"
    );
}

#[tokio::test]
async fn reviving_someone_elses_session_is_refused() {
    // Recovery paths are still access-controlled paths. This one takes an
    // identifier and starts a runtime, so it is worth pinning explicitly.
    let store = Arc::new(FakeStore::default());
    let alice = user("alice");
    let mallory = user("mallory");
    let svc = service(Arc::clone(&store), Arc::new(FakeAdapter::default()));

    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;
    svc.stop_session(&alice, &created.id)
        .await
        .expect("stopped");

    assert!(
        svc.revive_session(&mallory, &created.id).await.is_err(),
        "a session belongs to its owner, including when it is stopped"
    );
    assert!(
        svc.stop_session(&mallory, &created.id).await.is_err(),
        "and stopping is no more public than reviving"
    );
}

/// A store, a conversation-capable runtime, and the profile that puts a
/// session on the conversation plane — shared across two services, because
/// "restart" here means a second service over the same everything.
fn conversing_deployment() -> (
    Arc<FakeStore>,
    Arc<FakeAdapter>,
    Arc<crate::environment_service::EnvironmentProfileService>,
) {
    let profiles =
        crate::test_support::environments_defaulting_to(crate::environment::AgentConfiguration {
            kernel: crate::test_support::kernel(),
            mode: crate::options::SessionMode::Conversation,
            ..Default::default()
        });
    (
        Arc::new(FakeStore::default()),
        Arc::new(FakeAdapter {
            capabilities: crate::conversation_support::conversing(),
            ..FakeAdapter::default()
        }),
        profiles,
    )
}

#[tokio::test]
async fn a_conversation_session_is_taken_back_over_as_one() {
    // The §5 failure this closes, and the one that section did not anticipate:
    // not a deployment interrupting a session, but a deployment leaving it
    // listed, intact, and unusable. Readopt predates conversation mode and
    // attached a terminal to everything — which two of three kernels do not
    // have, and which leaves nothing to prompt even where it succeeds.
    let (store, adapter, profiles) = conversing_deployment();
    let alice = user("alice");

    let id = {
        let first = crate::test_support::service_with_environments(
            Arc::clone(&store),
            Arc::clone(&adapter),
            Arc::clone(&profiles),
        );
        let created = first.create_session(&alice, spec()).await.expect("created");
        await_settled(&first, &alice, &created.id).await;
        created.id
    };

    // The process is gone; the record and the runtime are not.
    let replacement = crate::test_support::service_with_environments(
        Arc::clone(&store),
        Arc::clone(&adapter),
        Arc::clone(&profiles),
    );
    // No revive: nothing was stopped. Prompting is what a person does next,
    // and it is where the runtime has to be picked back up — exactly as
    // attaching is for the terminal plane.
    replacement
        .send_prompt(&alice, &id, "still there?".to_string())
        .await
        .expect("a live runtime is taken back over, and the session prompted");
}

#[tokio::test]
async fn taking_a_conversation_back_over_rejoins_it_rather_than_opening_another() {
    // The half that is invisible without asking. The bridge in the pod
    // outlives a control plane restart, so opening a conversation rather than
    // rejoining one leaves a second running beside the first: the owner still
    // sees their history, because the durable log has it, and the agent has
    // lost everything it knew.
    let (store, adapter, profiles) = conversing_deployment();
    let alice = user("alice");

    let id = {
        let first = crate::test_support::service_with_environments(
            Arc::clone(&store),
            Arc::clone(&adapter),
            Arc::clone(&profiles),
        );
        let created = first.create_session(&alice, spec()).await.expect("created");
        await_settled(&first, &alice, &created.id).await;
        created.id
    };
    assert_eq!(
        adapter.rejoined.lock().await.as_slice(),
        &[None],
        "a session being started has no conversation to rejoin"
    );

    let replacement = crate::test_support::service_with_environments(
        Arc::clone(&store),
        Arc::clone(&adapter),
        Arc::clone(&profiles),
    );
    replacement
        .send_prompt(&alice, &id, "still there?".to_string())
        .await
        .expect("taken back over");

    let asked = adapter.rejoined.lock().await.clone();
    assert_eq!(
        asked.last().expect("a second open"),
        &Some("conv-fixture".to_string()),
        "the replacement must rejoin what the runtime is already running"
    );
}

#[tokio::test]
async fn a_session_recorded_before_capabilities_existed_is_settled_when_it_comes_back() {
    // The upgrade case, and the one that decides whether deploying the
    // capability model costs somebody a running session. A record written by
    // an older build has no granted set at all; read back, that is every
    // capability false — which every gate reads as a refusal, including the
    // one behind each keystroke.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");

    let first = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = first.create_session(&alice, spec()).await.expect("created");
    await_settled(&first, &alice, &created.id).await;

    // Exactly what an older build left behind: everything else intact, the
    // granted set absent.
    let mut record = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present");
    record.session.capabilities = Default::default();
    assert!(record.session.capabilities.unrecorded());
    store.upsert(&record).await.expect("recorded");

    drop(first);
    let restarted = service(Arc::clone(&store), Arc::clone(&adapter));
    let recovered = restarted.reconcile().await;

    assert_eq!(recovered.readopted_sessions, 1);
    let settled = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present")
        .session
        .capabilities;
    assert!(
        settled.terminal,
        "a terminal session must come back able to take a keystroke"
    );
    assert!(
        !settled.unrecorded(),
        "the settled set must be written down, not derived again by every process"
    );
    // Through the interface a caller actually uses, not only in the record.
    restarted
        .send_input(&alice, &created.id, b"ls\n".to_vec())
        .await
        .expect("a readopted session must accept input");
}

#[tokio::test]
async fn a_session_that_recorded_its_own_set_keeps_it() {
    // The other half: settling is for records that never had one. A session
    // that was granted its abilities deliberately must not have them
    // recomputed underneath it — that is the recomputation the whole model
    // exists to forbid.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");

    let first = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = first.create_session(&alice, spec()).await.expect("created");
    await_settled(&first, &alice, &created.id).await;

    let mut record = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present");
    // A set that could never be derived from this runtime, so a recomputation
    // is visible rather than coincidental.
    record.session.capabilities = crate::capabilities::SessionCapabilities {
        terminal: true,
        snapshot: false,
        ..Default::default()
    };
    store.upsert(&record).await.expect("recorded");

    drop(first);
    let restarted = service(Arc::clone(&store), Arc::clone(&adapter));
    restarted.reconcile().await;

    let after = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present")
        .session
        .capabilities;
    assert!(
        after.terminal && !after.snapshot,
        "a recorded set must survive being picked back up: {after:?}"
    );
}
