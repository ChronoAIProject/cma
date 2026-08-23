//! Tests for bringing a runtime up behind the request that asked for it.
//!
//! The point of provisioning asynchronously is that no connection has to be
//! held open for it. The risk it introduces is that nobody is listening when it
//! goes wrong, so most of what is asserted here is that failures end up
//! recorded rather than merely logged.

use std::sync::Arc;

use crate::session::{SessionStatus, StopReason};
use crate::store::SessionStore;
use crate::test_support::{await_settled, service, spec, user, FakeAdapter, FakeStore};

#[tokio::test]
async fn creating_a_session_returns_before_a_runtime_exists() {
    // The whole reason for the change: the caller is answered immediately, so
    // no timeout anywhere is load-bearing.
    let store = Arc::new(FakeStore::default());
    let caller = user("alice");
    let created = service(Arc::clone(&store), Arc::new(FakeAdapter::default()))
        .create_session(&caller, spec())
        .await
        .expect("created");

    assert_eq!(created.status, SessionStatus::Provisioning);
}

#[tokio::test]
async fn the_session_is_durable_before_any_runtime_is_asked_for() {
    // Record-first is what makes a leak impossible: a runtime cannot come to
    // exist before something references it.
    let store = Arc::new(FakeStore::default());
    let created = service(Arc::clone(&store), Arc::new(FakeAdapter::default()))
        .create_session(&user("alice"), spec())
        .await
        .expect("created");

    let record = store
        .get(&created.id)
        .await
        .expect("store readable")
        .expect("the record exists the moment creation returns");
    assert_eq!(record.session.status, SessionStatus::Provisioning);
}

#[tokio::test]
async fn a_session_becomes_ready_once_its_runtime_is_up() {
    let store = Arc::new(FakeStore::default());
    let caller = user("alice");
    let svc = service(Arc::clone(&store), Arc::new(FakeAdapter::default()));
    let created = svc.create_session(&caller, spec()).await.expect("created");

    let settled = await_settled(&svc, &caller, &created.id).await;
    assert_eq!(settled.status, SessionStatus::Ready);
}

#[tokio::test]
async fn a_provider_that_cannot_provision_fails_the_session_rather_than_leaving_it_pending() {
    // Nobody is waiting to be told. A session left in provisioning forever is
    // worse than the synchronous version, where the caller at least saw the
    // error, so the failure has to be written down.
    let store = Arc::new(FakeStore::default());
    let caller = user("alice");
    let adapter = Arc::new(FakeAdapter {
        refuse_provision: true,
        ..FakeAdapter::default()
    });
    let svc = service(Arc::clone(&store), adapter);
    let created = svc.create_session(&caller, spec()).await.expect("created");

    let settled = await_settled(&svc, &caller, &created.id).await;
    assert_eq!(settled.status, SessionStatus::Terminated);
    assert_eq!(settled.stop_reason, Some(StopReason::Error));
}

#[tokio::test]
async fn a_late_capacity_refusal_is_not_retried_and_is_recorded_for_the_owner() {
    let store = Arc::new(FakeStore::default());
    let caller = user("alice");
    let adapter = Arc::new(FakeAdapter {
        refuse_at_capacity: true,
        ..FakeAdapter::default()
    });
    let svc = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = svc.create_session(&caller, spec()).await.expect("created");

    let settled = await_settled(&svc, &caller, &created.id).await;
    assert_eq!(settled.status, SessionStatus::Terminated);
    assert_eq!(
        adapter
            .provision_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        1,
        "waiting and asking again cannot create room"
    );

    let events = svc
        .events_since(&caller, &created.id, None)
        .await
        .expect("history readable");
    let (detail, capability) = events
        .iter()
        .find_map(|event| match &event.kind {
            crate::event::EventKind::StatusChanged {
                status: SessionStatus::Terminated,
                detail: Some(detail),
                capability,
                ..
            } => Some((detail, capability)),
            _ => None,
        })
        .expect("the terminal status is explained");
    assert_eq!(
        detail,
        "This deployment has no room for a session this size right now. Nothing is wrong with your session or your request — stop a session, or ask for a smaller one, and try again."
    );
    assert_eq!(capability, &None);
    for forbidden in [
        "provider",
        "protocol",
        "kubernetes",
        "namespace",
        "quota",
        "http",
    ] {
        assert!(
            !detail.to_ascii_lowercase().contains(forbidden),
            "owner wording leaked {forbidden:?}: {detail}"
        );
    }
}

#[tokio::test]
async fn a_runtime_that_cannot_be_attached_to_is_torn_down() {
    // The leak this design exists to prevent: provisioning succeeded, so a
    // runtime is running, and the session it belongs to has failed.
    let store = Arc::new(FakeStore::default());
    let caller = user("alice");
    let adapter = Arc::new(FakeAdapter {
        refuse_attach: true,
        ..FakeAdapter::default()
    });
    let svc = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = svc.create_session(&caller, spec()).await.expect("created");

    let settled = await_settled(&svc, &caller, &created.id).await;
    assert_eq!(settled.status, SessionStatus::Terminated);
    let torn_down = adapter.terminated.lock().await.clone();
    assert!(
        !torn_down.is_empty() && torn_down.iter().all(|r| r.contains(&created.id.0)),
        "every runtime the attempts created must be torn down, got {torn_down:?}"
    );
}

#[tokio::test]
async fn a_failed_session_stops_referencing_a_runtime_that_is_gone() {
    // Otherwise reconciliation would keep trying to reclaim something already
    // torn down.
    let store = Arc::new(FakeStore::default());
    let caller = user("alice");
    let svc = service(
        Arc::clone(&store),
        Arc::new(FakeAdapter {
            refuse_attach: true,
            ..FakeAdapter::default()
        }),
    );
    let created = svc.create_session(&caller, spec()).await.expect("created");
    await_settled(&svc, &caller, &created.id).await;

    let record = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present");
    assert!(record.runtime_id.is_none());
}

#[tokio::test]
async fn the_failure_is_appended_to_the_durable_log() {
    // A client following the event stream has to learn that it failed; polling
    // the record is not the only way this is observed.
    let store = Arc::new(FakeStore::default());
    let caller = user("alice");
    let svc = service(
        Arc::clone(&store),
        Arc::new(FakeAdapter {
            refuse_provision: true,
            ..FakeAdapter::default()
        }),
    );
    let created = svc.create_session(&caller, spec()).await.expect("created");
    await_settled(&svc, &caller, &created.id).await;

    let events = svc
        .events_since(&caller, &created.id, None)
        .await
        .expect("history readable");
    let seqs: Vec<u64> = events.iter().map(|e| e.seq).collect();
    let mut sorted = seqs.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(seqs, sorted, "sequences must stay monotonic and unique");
    assert!(
        events.len() >= 3,
        "creation, provisioning and the failure must all be recorded, got {}",
        events.len()
    );
}

#[tokio::test]
async fn an_unknown_kernel_is_refused_immediately_rather_than_asynchronously() {
    // Named by what the session is started from rather than by the request,
    // now that a caller does not choose a kernel. Still refused while somebody
    // is on the line to be told: accepting it would mean writing a record for
    // a session that can never start.
    let store = Arc::new(FakeStore::default());
    let unstartable =
        crate::test_support::environments_defaulting_to(crate::environment::AgentConfiguration {
            kernel: crate::ids::KernelId::new("no-such-kernel"),
            ..Default::default()
        });
    let result = crate::test_support::service_with_environments(
        Arc::clone(&store),
        Arc::new(FakeAdapter::default()),
        unstartable,
    )
    .create_session(&user("alice"), spec())
    .await;

    assert!(result.is_err());
    assert!(
        store.list_all().await.expect("readable").is_empty(),
        "a refused request must leave no record behind"
    );
}

#[tokio::test]
async fn a_transient_provider_failure_is_retried_rather_than_reported() {
    // Someone asking for a session wants a session. A provider under load is
    // nobody's decision and usually passes, so it must not be the answer they
    // get — this is the failure that ended sessions with no explanation.
    let store = Arc::new(FakeStore::default());
    let caller = user("alice");
    let adapter = Arc::new(FakeAdapter {
        fail_first: std::sync::atomic::AtomicUsize::new(2),
        ..FakeAdapter::default()
    });
    let svc = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = svc.create_session(&caller, spec()).await.expect("created");

    let settled = await_settled(&svc, &caller, &created.id).await;
    assert_eq!(
        settled.status,
        SessionStatus::Ready,
        "a session must survive a provider that failed twice"
    );
}

#[tokio::test]
async fn a_failed_attempt_does_not_leave_a_runtime_behind() {
    // Each retry starts clean. Without discarding what the previous attempt
    // created, a session that eventually succeeds costs several sandboxes.
    let store = Arc::new(FakeStore::default());
    let caller = user("alice");
    let adapter = Arc::new(FakeAdapter {
        // Provisioning succeeds and attaching fails, so a runtime exists each
        // time the attempt is abandoned.
        refuse_attach: true,
        ..FakeAdapter::default()
    });
    let svc = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = svc.create_session(&caller, spec()).await.expect("created");
    await_settled(&svc, &caller, &created.id).await;

    let torn_down = adapter.terminated.lock().await.len();
    assert!(
        torn_down >= 2,
        "every runtime a failed attempt created must be torn down, got {torn_down}"
    );
}

#[tokio::test]
async fn a_session_that_cannot_start_says_why() {
    // The reason travels with the event, so the person watching learns what
    // the log already knew instead of seeing a session end for no stated
    // reason.
    let store = Arc::new(FakeStore::default());
    let caller = user("alice");
    let svc = service(
        Arc::clone(&store),
        Arc::new(FakeAdapter {
            refuse_provision: true,
            ..FakeAdapter::default()
        }),
    );
    let created = svc.create_session(&caller, spec()).await.expect("created");
    await_settled(&svc, &caller, &created.id).await;

    let events = svc
        .events_since(&caller, &created.id, None)
        .await
        .expect("history readable");
    let explained = events.iter().any(|e| {
        matches!(&e.kind, crate::event::EventKind::StatusChanged { detail: Some(d), .. }
            if d.contains("no capacity") || d.contains("attempts"))
    });
    assert!(explained, "the failure carried no explanation: {events:?}");
}

#[tokio::test]
async fn every_event_of_a_successful_start_is_recorded() {
    // Progress and status were allocated by two counters from the same
    // starting point, so they claimed the same sequence and the second insert
    // was rejected. The session came up and nothing recorded that it had,
    // leaving a live session that looked stuck forever.
    let store = Arc::new(FakeStore::default());
    let caller = user("alice");
    let svc = service(Arc::clone(&store), Arc::new(FakeAdapter::default()));
    let created = svc.create_session(&caller, spec()).await.expect("created");
    await_settled(&svc, &caller, &created.id).await;

    let events = svc
        .events_since(&caller, &created.id, None)
        .await
        .expect("history readable");

    let seqs: Vec<u64> = events.iter().map(|e| e.seq).collect();
    let mut unique = seqs.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        seqs.len(),
        unique.len(),
        "two events claimed the same sequence: {seqs:?}"
    );

    // The one that matters: a client following the stream has to be told the
    // session is usable.
    let announced_ready = events.iter().any(|e| {
        matches!(
            &e.kind,
            crate::event::EventKind::StatusChanged {
                status: SessionStatus::Ready,
                ..
            }
        )
    });
    assert!(announced_ready, "readiness was never recorded: {events:?}");
}

#[tokio::test]
async fn asking_for_the_owners_own_credential_fails_the_session_and_says_why() {
    // The defect this closes was silent: the option was accepted end to end
    // and honoured nowhere, so a session ran on the delegated credential while
    // its owner believed it ran on theirs. What replaces it has to be
    // attributable, which means the reason reaching the log the owner reads.
    let store = Arc::new(FakeStore::default());
    let caller = user("alice");
    let adapter = Arc::new(FakeAdapter::default());
    let own_credential =
        crate::test_support::environments_defaulting_to(crate::environment::AgentConfiguration {
            kernel: crate::test_support::kernel(),
            credential: crate::options::CredentialMode::Own,
            ..Default::default()
        });
    let svc = crate::test_support::service_with_environments(
        Arc::clone(&store),
        Arc::clone(&adapter),
        own_credential,
    );
    let created = svc.create_session(&caller, spec()).await.expect("created");

    let settled = await_settled(&svc, &caller, &created.id).await;
    assert_eq!(settled.status, SessionStatus::Terminated);
    assert_eq!(settled.stop_reason, Some(StopReason::Error));

    let events = svc
        .events_since(&caller, &created.id, None)
        .await
        .expect("history readable");
    let explained = events.iter().any(|e| match &e.kind {
        crate::event::EventKind::StatusChanged {
            detail: Some(detail),
            ..
        } => detail.contains("own credential"),
        _ => false,
    });
    assert!(
        explained,
        "the refusal must name what was refused, not a bare failure: {events:?}"
    );
    assert_eq!(
        adapter
            .provision_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        1,
        "an absent ability is an answer; retrying it only delays telling the owner"
    );
}

#[tokio::test]
async fn the_delegated_credential_still_starts_a_session() {
    // The success path, so the refusal above cannot quietly widen to every
    // session: the default credential mode is unaffected.
    let store = Arc::new(FakeStore::default());
    let caller = user("alice");
    let svc = service(Arc::clone(&store), Arc::new(FakeAdapter::default()));
    let created = svc.create_session(&caller, spec()).await.expect("created");

    assert_eq!(
        await_settled(&svc, &caller, &created.id).await.status,
        SessionStatus::Ready
    );
}

#[tokio::test]
async fn a_session_its_kernel_cannot_be_is_refused_while_someone_is_listening() {
    // The point of the capability model: the refusal arrives as the answer to
    // the request that asked for it, naming the ability that is missing.
    // Discovering it afterwards — an empty stream, a broken screen — is the
    // failure this replaces.
    let store = Arc::new(FakeStore::default());
    let terminal_only =
        crate::test_support::environments_defaulting_to(crate::environment::AgentConfiguration {
            kernel: crate::test_support::kernel(),
            mode: crate::options::SessionMode::Conversation,
            ..Default::default()
        });
    let adapter = Arc::new(FakeAdapter::default());
    let result = crate::test_support::service_with_environments(
        Arc::clone(&store),
        Arc::clone(&adapter),
        terminal_only,
    )
    .create_session(&user("alice"), spec())
    .await;

    assert_eq!(
        result.expect_err("a conversation on a terminal-only kernel cannot exist"),
        crate::error::CoreError::CapabilityUnsupported("conversation".into())
    );
    assert!(
        store.list_all().await.expect("readable").is_empty(),
        "a refused request must leave no record behind"
    );
    assert_eq!(
        adapter
            .provision_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "nothing may be provisioned for a session that cannot be what it asked to be"
    );
}

#[tokio::test]
async fn a_session_carries_the_set_it_was_granted() {
    // Recorded, not recomputed: what a session may be asked to do has to be
    // the same tomorrow, whatever its kernel's implementation does next.
    let store = Arc::new(FakeStore::default());
    let caller = user("alice");
    let svc = service(Arc::clone(&store), Arc::new(FakeAdapter::default()));
    let created = svc.create_session(&caller, spec()).await.expect("created");

    assert!(created.capabilities.terminal);
    assert!(created.capabilities.snapshot);
    assert!(!created.capabilities.conversation);
    assert!(
        !created.capabilities.usage && !created.capabilities.steer,
        "a terminal session has no turns to steer and no tokens to count"
    );

    let stored = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present");
    assert_eq!(stored.session.capabilities, created.capabilities);
}

#[tokio::test]
async fn a_kernel_that_stops_offering_what_a_session_needs_does_not_start_it() {
    // The case a granted set exists to catch: the session was legitimate when
    // it was created, and its kernel has changed underneath it since. Bringing
    // it back anyway would leave a record claiming abilities the runtime does
    // not have, which every caller downstream trusts cannot happen.
    let store = Arc::new(FakeStore::default());
    let caller = user("alice");
    let adapter = Arc::new(FakeAdapter::default());
    let before = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = before
        .create_session(&caller, spec())
        .await
        .expect("created");
    await_settled(&before, &caller, &created.id).await;
    drop(before);

    // The sandbox is gone, so the session is one the controller would restart
    // — onto a build of its kernel that no longer renders a terminal.
    let handle = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present")
        .runtime_id
        .expect("has a runtime");
    adapter.lose(&handle).await;
    let screenless = Arc::new(FakeAdapter {
        capabilities: layer3_adapter::KernelCapabilities::default(),
        ..FakeAdapter::default()
    });
    let after = service(Arc::clone(&store), Arc::clone(&screenless));
    after.reconcile().await;

    let settled = await_settled(&after, &caller, &created.id).await;
    assert_eq!(settled.status, SessionStatus::Terminated);
    assert_eq!(
        screenless
            .provision_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "the check happens before a runtime is asked for"
    );

    // And the refusal is in the log by name. It was arrived at typed — the
    // core knew it was the terminal that was missing — and until this it was
    // flattened into a sentence on the way out, which left every interface
    // above able to say only that something had gone wrong.
    let logged = after
        .events_since(&caller, &created.id, None)
        .await
        .expect("the log is readable");
    let named = logged
        .iter()
        .filter_map(|e| match &e.kind {
            crate::event::EventKind::StatusChanged {
                status: SessionStatus::Terminated,
                capability,
                detail,
                ..
            } => Some((capability.clone(), detail.clone())),
            _ => None,
        })
        .next()
        .expect("the session was terminated, so the log says so");
    assert_eq!(
        named.0.as_deref(),
        Some("terminal"),
        "the ability that was missing is named, not described: {named:?}"
    );
    assert!(
        named.1.is_some(),
        "and the sentence for a person is still there: {named:?}"
    );
}
