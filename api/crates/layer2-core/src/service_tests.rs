//! Tests for the session service's terminal relay.
//!
//! Kept in a sibling file so `service.rs` stays within the project's file-size
//! limit while the behaviour remains covered.

use super::*;
use crate::terminal::{ScreenSnapshot, TerminalGeometry};
use crate::terminal_io::spawn_relay;
use crate::test_support::{service, spec, user, FakeAdapter, FakeStore};
use layer3_adapter::TerminalOutput;

/// A screen model that records what it was fed, so the relay's behaviour
/// can be observed without a real terminal parser.
struct RecordingScreen {
    geometry: TerminalGeometry,
    seen: Vec<u8>,
}

impl ScreenModel for RecordingScreen {
    fn apply(&mut self, bytes: &[u8]) {
        self.seen.extend_from_slice(bytes);
    }

    fn resize(&mut self, geometry: TerminalGeometry) {
        self.geometry = geometry;
    }

    fn snapshot(&self) -> ScreenSnapshot {
        ScreenSnapshot {
            geometry: self.geometry,
            rows: vec![String::from_utf8_lossy(&self.seen).into_owned()],
        }
    }

    fn repaint_bytes(&self) -> Vec<u8> {
        self.seen.clone()
    }
}

#[tokio::test]
async fn the_relay_feeds_the_screen_with_no_client_attached() {
    // The whole point of the relay: a detached session keeps working, and
    // its screen stays current so a later attach is correct.
    let service = service(
        Arc::new(FakeStore::default()),
        Arc::new(FakeAdapter::default()),
    );
    let (tx, rx) = mpsc::channel::<TerminalOutput>(8);
    let geometry = TerminalGeometry::default();
    let screen: Arc<Mutex<Box<dyn ScreenModel>>> =
        Arc::new(Mutex::new(Box::new(RecordingScreen {
            geometry,
            seen: Vec::new(),
        })));
    let (live, _) = broadcast::channel(8);
    let (ending, _) = tokio::sync::watch::channel(None);

    spawn_relay(
        service,
        SessionId("sesn_test".into()),
        rx,
        Arc::clone(&screen),
        live,
        ending,
    );

    tx.send(TerminalOutput::Bytes(b"painted".to_vec()))
        .await
        .expect("relay accepts output");
    // Yield until the relay has drained the channel.
    for _ in 0..50 {
        if !screen.lock().await.repaint_bytes().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(
        screen.lock().await.repaint_bytes(),
        b"painted".to_vec(),
        "the screen must absorb output even with nobody attached"
    );
}

#[tokio::test]
async fn output_with_no_subscriber_is_not_an_error() {
    // A send into a broadcast with no receivers fails; the relay must treat
    // that as "detached", not as a fault that ends the session.
    let service = service(
        Arc::new(FakeStore::default()),
        Arc::new(FakeAdapter::default()),
    );
    let (tx, rx) = mpsc::channel::<TerminalOutput>(8);
    let screen: Arc<Mutex<Box<dyn ScreenModel>>> =
        Arc::new(Mutex::new(Box::new(RecordingScreen {
            geometry: TerminalGeometry::default(),
            seen: Vec::new(),
        })));
    let (live, _) = broadcast::channel(8);
    let (ending, _) = tokio::sync::watch::channel(None);
    spawn_relay(
        service,
        SessionId("sesn_test".into()),
        rx,
        Arc::clone(&screen),
        live,
        ending,
    );

    for chunk in [b"one".to_vec(), b"two".to_vec()] {
        tx.send(TerminalOutput::Bytes(chunk))
            .await
            .expect("relay still accepting");
    }
    for _ in 0..50 {
        if screen.lock().await.repaint_bytes().len() >= 6 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(screen.lock().await.repaint_bytes(), b"onetwo".to_vec());
}

#[tokio::test]
async fn a_session_runs_on_the_kernel_this_deployment_configured() {
    // A caller cannot name one, and the type is what says so: the field a
    // caller would have filled is gone, so an interface cannot reach past the
    // decision by substituting a literal — which is what both of them did, and
    // what the core then silently overwrote.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let profiles =
        crate::test_support::environments_defaulting_to(crate::environment::AgentConfiguration {
            kernel: crate::test_support::kernel(),
            ..Default::default()
        });
    let svc = crate::test_support::service_with_environments(
        Arc::clone(&store),
        Arc::clone(&adapter),
        profiles,
    );
    let alice = user("alice");

    let created = svc.create_session(&alice, spec()).await.expect("created");
    assert_eq!(
        created.kernel,
        crate::test_support::kernel(),
        "the deployment's default is what an unspecified session gets"
    );
}

#[tokio::test]
async fn a_kernel_this_deployment_does_not_run_is_refused_before_anything_is_recorded() {
    // The profile is where a kernel is named, so a profile naming one this
    // deployment cannot start must fail the request rather than leave a session
    // provisioning towards a runtime that does not exist.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let profiles =
        crate::test_support::environments_defaulting_to(crate::environment::AgentConfiguration {
            kernel: crate::ids::KernelId::new("a-kernel-this-deployment-does-not-run"),
            ..Default::default()
        });
    let svc = crate::test_support::service_with_environments(
        Arc::clone(&store),
        Arc::clone(&adapter),
        profiles,
    );
    let alice = user("alice");

    let err = svc
        .create_session(&alice, spec())
        .await
        .expect_err("a kernel this deployment cannot start must not be accepted");
    assert!(
        matches!(err, CoreError::RuntimeUnavailable(_)),
        "got {err:?}"
    );
    assert!(
        svc.list_sessions(&alice, &crate::SessionListQuery::default())
            .await
            .expect("session list is readable")
            .items
            .is_empty(),
        "nothing may be recorded for a session that was refused"
    );
}

#[tokio::test]
async fn a_runtime_is_advertised_as_what_it_says_it_can_be() {
    // What a client is offered comes from the adapter, not from a list kept
    // beside it: a kernel that gains the conversation plane is offered for it
    // the moment it says so, and one that loses a plane stops being offered
    // without anything here being edited.
    let store = Arc::new(FakeStore::default());
    let screen_only = Arc::new(FakeAdapter::default());
    let svc = crate::test_support::service(Arc::clone(&store), screen_only);

    assert_eq!(
        svc.available_kernels(),
        vec![(crate::test_support::kernel(), vec![SessionMode::Terminal])],
        "the fake runtime renders a screen and holds no conversation"
    );

    let both = Arc::new(FakeAdapter {
        capabilities: layer3_adapter::KernelCapabilities {
            conversation: true,
            ..FakeAdapter::default().capabilities
        },
        ..FakeAdapter::default()
    });
    let svc = crate::test_support::service(Arc::clone(&store), both);

    assert_eq!(
        svc.available_kernels(),
        vec![(
            crate::test_support::kernel(),
            vec![SessionMode::Terminal, SessionMode::Conversation]
        )],
    );
}

#[tokio::test]
async fn an_interface_that_only_holds_conversations_can_create_one() {
    // The plane a caller asked for was discarded along with everything else in
    // `options`, which is filled in from the profile or the deployment. On a
    // deployment defaulting to terminal that meant AG-UI created a terminal
    // session and then refused it for lacking the conversation ability it had
    // just asked for — so the priority-1 interface could not create a session
    // it could use, unless the caller had hand-made a profile first.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter {
        capabilities: crate::conversation_support::conversing(),
        ..FakeAdapter::default()
    });
    let svc = crate::test_support::service_with_environments(
        Arc::clone(&store),
        Arc::clone(&adapter),
        crate::test_support::environments_defaulting_to(crate::environment::AgentConfiguration {
            kernel: crate::test_support::kernel(),
            mode: crate::options::SessionMode::Terminal,
            ..Default::default()
        }),
    );
    let alice = user("alice");

    let created = svc
        .create_session(
            &alice,
            SessionSpec {
                requested_mode: Some(crate::options::SessionMode::Conversation),
                ..spec()
            },
        )
        .await
        .expect("created");

    assert_eq!(
        created.options.mode,
        crate::options::SessionMode::Conversation
    );
    assert!(
        created.capabilities.conversation,
        "and it is granted the ability, or the very next call refuses it"
    );
}

#[tokio::test]
async fn a_caller_who_says_nothing_still_gets_the_deployments_default() {
    // The narrowness matters. An interface with no way to express a plane must
    // keep getting what it always got.
    let store = Arc::new(FakeStore::default());
    let svc = crate::test_support::service_with_environments(
        Arc::clone(&store),
        Arc::new(FakeAdapter::default()),
        crate::test_support::environments_defaulting_to(crate::environment::AgentConfiguration {
            kernel: crate::test_support::kernel(),
            mode: crate::options::SessionMode::Terminal,
            ..Default::default()
        }),
    );
    let alice = user("alice");

    let created = svc
        .create_session(
            &alice,
            SessionSpec {
                requested_mode: None,
                ..spec()
            },
        )
        .await
        .expect("created");

    assert_eq!(created.options.mode, crate::options::SessionMode::Terminal);
}
