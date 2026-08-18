//! Running a kernel in a sandbox, whichever kernel it is.
//!
//! Nothing here names one. That is the point of the file: everything it covers
//! — the shape a sandbox is asked for, the link back, the credential that
//! rides on it, the wire the two ends settle on — was written once per kernel
//! before it was written once, and a test that mentioned a kernel would be
//! evidence the split had not actually happened.

#![allow(clippy::items_after_test_module)]

use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use super::*;
use crate::supervisor;
use crate::{RuntimeSpec, TermSize};

/// A hosted kernel talking to a provider at `base`.
fn hosted(base: &str) -> HostedKernel {
    HostedKernel::new(
        SandboxClient::new(base, "test-key"),
        SandboxShape::default(),
        "test-kernel",
    )
}

/// A provisioning request for a session with nothing carried into it.
fn spec() -> RuntimeSpec {
    RuntimeSpec {
        restore_from: None,
        reaches: Vec::new(),
        created_by: None,
        environment: Default::default(),
        session_id: "sesn_x".into(),
        size: TermSize { cols: 80, rows: 24 },
        subject_credential: None,
        options: Default::default(),
    }
}

#[test]
fn a_proxied_endpoint_becomes_a_link_url() {
    let k = hosted("http://provider:80");
    assert_eq!(
        k.link_url("provider/v1/sandboxes/sbx-1/proxy/9090", "sbx-1"),
        "ws://provider/v1/sandboxes/sbx-1/proxy/9090/runtime/sbx-1/terminal?wire=1"
    );
}

#[test]
fn a_secure_provider_yields_a_secure_socket() {
    let k = hosted("https://provider");
    assert!(k
        .link_url("provider/v1/sandboxes/s/proxy/9090", "s")
        .starts_with("wss://"));
}

#[test]
fn an_endpoint_that_already_has_a_scheme_is_not_doubled() {
    let k = hosted("http://provider");
    assert_eq!(
        k.link_url("http://provider/proxy/9090/", "s"),
        "ws://provider/proxy/9090/runtime/s/terminal?wire=1"
    );
}

#[test]
fn a_link_asks_for_the_wire_it_speaks() {
    // Asked for rather than assumed. A pod that predates lanes ignores this
    // and keeps speaking what it always did, which is what makes a
    // half-rolled-out fleet safe.
    let k = hosted("http://provider:80");
    let url = k.link_url("provider/v1/sandboxes/sbx-1/proxy/9090", "sbx-1");
    assert!(
        url.ends_with(&format!(
            "/runtime/sbx-1/terminal?wire={}",
            supervisor::WIRE_VERSION
        )),
        "got {url}"
    );
}

#[test]
fn the_default_shape_requests_a_real_workload_size() {
    // A default, not a policy: the deployment sets these, because how much a
    // session costs is decided against the quota it is drawn from.
    let s = SandboxShape::default();
    assert_eq!(s.resource_limits.get("cpu").map(String::as_str), Some("1"));
    assert_eq!(
        s.resource_limits.get("memory").map(String::as_str),
        Some("1Gi")
    );
}

#[test]
fn sandboxes_do_not_expire_on_a_timer() {
    // A session is someone's working terminal. Expiring it on a timer ends
    // live work mid-turn, and a timer cannot tell an idle session from one
    // whose owner is reading the screen. Only the owner ends a session.
    assert!(
        SandboxShape::default().timeout_seconds.is_none(),
        "a default lifetime would terminate active sessions"
    );
}

#[tokio::test]
async fn provisioning_against_a_dead_provider_reports_unavailable() {
    // Port 1 is reserved and never listening, so this exercises the failure
    // path deterministically without a fixture server.
    let k = hosted("http://127.0.0.1:1");
    let err = k
        .provision(&spec(), BTreeMap::new())
        .await
        .expect_err("must fail against a dead provider");
    assert!(matches!(err, AdapterError::Unavailable(_)), "got {err:?}");
}

#[tokio::test]
async fn attaching_against_a_dead_provider_reports_unavailable() {
    let k = hosted("http://127.0.0.1:1");
    let err = k
        .attach_terminal(&RuntimeHandle("sbx-1".into()))
        .await
        .expect_err("must fail against a dead provider");
    assert!(matches!(err, AdapterError::Unavailable(_)), "got {err:?}");
}

#[tokio::test]
async fn terminating_against_a_dead_provider_is_still_an_error() {
    // The provider answers 404 for a sandbox already gone, and tearing down
    // something that no longer exists is a success; that tolerance is the
    // client's and is covered there. Unreachable is a different thing.
    let k = hosted("http://127.0.0.1:1");
    assert!(k.terminate(&RuntimeHandle("gone".into())).await.is_err());
}

#[tokio::test]
async fn a_credential_reaches_a_runtime_over_the_link_it_already_has() {
    // The frame is unchanged, which is the point: a pod started before this
    // method existed still understands what arrives, so no live session has to
    // be replaced to keep its credential fresh.
    let k = hosted("http://127.0.0.1:1");
    let handle = RuntimeHandle("sbx-1".into());
    let (tx, mut rx) = mpsc::channel(4);
    k.remember_link(&handle.0, tx).await;

    k.refresh_credential(&handle, "a-fresh-credential")
        .await
        .expect("a live link carries a credential");

    match rx.recv().await.expect("the frame was sent") {
        TerminalInput::Credential(value) => assert_eq!(value, "a-fresh-credential"),
        other => panic!("expected a credential frame, got {other:?}"),
    }
}

#[tokio::test]
async fn a_credential_for_a_runtime_with_no_link_is_refused_rather_than_dropped() {
    // Silence here would leave a session that looks alive and refuses every
    // call the agent makes on its owner's behalf, with nothing said about why.
    let k = hosted("http://127.0.0.1:1");
    let err = k
        .refresh_credential(&RuntimeHandle("never-attached".into()), "unused")
        .await
        .expect_err("there is nowhere for this to go");
    assert!(matches!(err, AdapterError::Unavailable(_)), "got {err:?}");
}

#[tokio::test]
async fn a_link_that_closed_is_forgotten_rather_than_retried_forever() {
    let k = hosted("http://127.0.0.1:1");
    let handle = RuntimeHandle("sbx-1".into());
    let (tx, rx) = mpsc::channel(1);
    drop(rx);
    k.remember_link(&handle.0, tx).await;

    assert!(k.refresh_credential(&handle, "unused").await.is_err());
    assert!(
        k.link_to(&handle.0).await.is_none(),
        "a dead link kept in the map would fail every renewal the same way forever"
    );
}

#[tokio::test]
async fn tearing_a_runtime_down_forgets_its_link() {
    // Otherwise this map grows with the remains of every session the process
    // has ever served.
    let k = hosted("http://127.0.0.1:1");
    let handle = RuntimeHandle("sbx-1".into());
    let (tx, _rx) = mpsc::channel(1);
    k.remember_link(&handle.0, tx).await;

    // Against a dead provider the delete fails; the link is dropped first, on
    // purpose, because the sandbox is going either way.
    let _ = k.terminate(&handle).await;
    assert!(k.link_to(&handle.0).await.is_none());
}

#[tokio::test]
async fn a_runtime_that_declares_its_wire_gets_the_framed_dialect() {
    let mut said = futures_util::stream::iter(vec![Ok(Message::Text(
        r#"{"t":"ready","wireVersion":1,"lanes":["ctl","term"]}"#.into(),
    ))]);
    let (framed, early) = HostedKernel::negotiate(&mut said).await;
    assert!(framed);
    assert!(early.is_none());
}

#[tokio::test]
async fn a_runtime_that_starts_relaying_is_speaking_the_older_wire() {
    // And the bytes it sent while answering are the top of somebody's screen,
    // so they are carried rather than dropped on the way to the answer.
    let mut said = futures_util::stream::iter(vec![Ok(Message::Binary(
        b"welcome to the terminal".to_vec(),
    ))]);
    let (framed, early) = HostedKernel::negotiate(&mut said).await;
    assert!(!framed, "an old pod never declares anything");
    assert_eq!(
        early.as_deref(),
        Some(b"welcome to the terminal".as_slice())
    );
}

#[tokio::test]
async fn a_runtime_that_says_nothing_at_all_is_the_older_wire_too() {
    // The bounded wait, settled the only way that cannot corrupt a session.
    let mut said = futures_util::stream::iter(Vec::<
        Result<Message, tokio_tungstenite::tungstenite::Error>,
    >::new());
    let (framed, early) = HostedKernel::negotiate(&mut said).await;
    assert!(!framed);
    assert!(early.is_none());
}

#[tokio::test]
async fn an_unfamiliar_control_frame_is_not_read_as_a_declaration() {
    let mut said = futures_util::stream::iter(vec![Ok(Message::Text(r#"{"t":"pong"}"#.into()))]);
    let (framed, _) = HostedKernel::negotiate(&mut said).await;
    assert!(
        !framed,
        "switching the wire on anything but a declaration would corrupt a pod that never agreed"
    );
}

#[test]
fn terminal_input_is_framed_only_on_the_wire_that_has_lanes() {
    // Control travels as text and is identical on both dialects, which is what
    // lets a credential reach a pod that predates lanes and one that does not
    // without either being told which it is talking to.
    let framed = HostedKernel::encode(TerminalInput::Bytes(b"ls\n".to_vec()), true);
    let Message::Binary(bytes) = framed else {
        panic!("terminal bytes travel as binary");
    };
    assert_eq!(
        supervisor::unframe(&bytes),
        Some((supervisor::LANE_TERMINAL, b"ls\n".as_slice()))
    );

    let bare = HostedKernel::encode(TerminalInput::Bytes(b"ls\n".to_vec()), false);
    assert_eq!(bare, Message::Binary(b"ls\n".to_vec()));
}

#[test]
fn a_runtime_that_answers_not_implemented_is_stating_an_absence() {
    // Read as a rejection it would look like something going wrong every time
    // rotation asked a runtime that was never able to answer.
    let absent: AdapterError = crate::sandbox::SandboxError::Rejected {
        status: 501,
        detail: "no".into(),
    }
    .into();
    assert!(matches!(absent, AdapterError::Unsupported(_)), "{absent:?}");

    let refused: AdapterError = crate::sandbox::SandboxError::Rejected {
        status: 403,
        detail: "no".into(),
    }
    .into();
    assert!(matches!(refused, AdapterError::Rejected(_)), "{refused:?}");
}
