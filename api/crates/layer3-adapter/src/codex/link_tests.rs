//! The link to a running runtime: its wire, its credential, its conversation.
//!
//! Split from the adapter's other tests so each file stays within the
//! project's size limit, on the division the code already has: this is about
//! talking to a runtime that exists.

#![allow(clippy::items_after_test_module)]

use std::collections::BTreeMap;

use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::codex::{CodexAdapter, CodexSandboxSpec};
use crate::sandbox::SandboxClient;
use crate::supervisor;
use crate::{AdapterError, KernelAdapter, RuntimeHandle, RuntimeSpec, TermSize, TerminalInput};

fn adapter(base: &str) -> CodexAdapter {
    CodexAdapter::new(
        SandboxClient::new(base, "test-key"),
        CodexSandboxSpec::default(),
    )
}

/// A spec, with the session's own environment attached.
fn spec_with(environment: crate::RuntimeEnvironment) -> RuntimeSpec {
    RuntimeSpec {
        restore_from: None,
        created_by: None,
        environment,
        session_id: "sesn_x".into(),
        size: TermSize { cols: 80, rows: 24 },
        subject_credential: None,
        options: Default::default(),
    }
}

#[tokio::test]
async fn a_credential_reaches_a_runtime_over_the_link_it_already_has() {
    // The frame is unchanged, which is the point: a pod started before this
    // method existed still understands what arrives, so no live session has to
    // be replaced to keep its credential fresh.
    let a = adapter("http://127.0.0.1:1");
    let handle = RuntimeHandle("sbx-1".into());
    let (tx, mut rx) = mpsc::channel(4);
    a.links.lock().await.insert(handle.0.clone(), tx);

    a.refresh_credential(&handle, "a-fresh-credential")
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
    let a = adapter("http://127.0.0.1:1");
    let err = a
        .refresh_credential(&RuntimeHandle("never-attached".into()), "unused")
        .await
        .expect_err("there is nowhere for this to go");
    assert!(matches!(err, AdapterError::Unavailable(_)), "got {err:?}");
}

#[tokio::test]
async fn a_link_that_closed_is_forgotten_rather_than_retried_forever() {
    let a = adapter("http://127.0.0.1:1");
    let handle = RuntimeHandle("sbx-1".into());
    let (tx, rx) = mpsc::channel(1);
    drop(rx);
    a.links.lock().await.insert(handle.0.clone(), tx);

    assert!(a.refresh_credential(&handle, "unused").await.is_err());
    assert!(
        !a.links.lock().await.contains_key(&handle.0),
        "a dead link kept in the map would fail every renewal the same way forever"
    );
}

#[tokio::test]
async fn tearing_a_runtime_down_forgets_its_link() {
    // Otherwise this map grows with the remains of every session the process
    // has ever served.
    let a = adapter("http://127.0.0.1:1");
    let handle = RuntimeHandle("sbx-1".into());
    let (tx, _rx) = mpsc::channel(1);
    a.links.lock().await.insert(handle.0.clone(), tx);

    // Against a dead provider the delete fails; the link is dropped first, on
    // purpose, because the sandbox is going either way.
    let _ = a.terminate(&handle).await;
    assert!(!a.links.lock().await.contains_key(&handle.0));
}

#[tokio::test]
async fn a_conversation_against_a_dead_provider_reports_unavailable() {
    // A conversation that could not be opened is a failure, not an absent
    // ability: this kernel does hold one, and a caller has to be able to tell
    // "could not reach it" from "never could".
    let a = adapter("http://127.0.0.1:1");
    let err = a
        .open_conversation(&RuntimeHandle("sbx-1".into()), &Default::default())
        .await
        .expect_err("a dead provider has no conversation to open");
    assert!(matches!(err, AdapterError::Unavailable(_)), "got {err:?}");
}

#[test]
fn a_sessions_own_variables_reach_the_sandbox() {
    // The defect this closes: a tenant set variables on a profile, the REST
    // surface accepted them, and nothing ever reached the pod.
    let a = adapter("http://127.0.0.1:1");
    let env = a.environment_for(&spec_with(crate::RuntimeEnvironment {
        variables: BTreeMap::from([("MY_TOKEN_NAME".to_string(), "a-value".to_string())]),
        ..Default::default()
    }));
    assert_eq!(
        env.get("MY_TOKEN_NAME").map(String::as_str),
        Some("a-value")
    );
}

#[test]
fn a_name_the_runtime_controls_cannot_be_taken_over() {
    // These names are how the runtime finds its upstream, its proxy and its
    // credential. A caller who could overwrite one could redirect the agent's
    // traffic — and the credential attached to it — somewhere of their
    // choosing. The refusal is logged by `apply_to`.
    let a = adapter("http://127.0.0.1:1");
    let env = a.environment_for(&spec_with(crate::RuntimeEnvironment {
        variables: BTreeMap::from([(
            "CHRONO_UPSTREAM_BASE_URL".to_string(),
            "http://somewhere-else.example".to_string(),
        )]),
        ..Default::default()
    }));
    assert_ne!(
        env.get("CHRONO_UPSTREAM_BASE_URL").map(String::as_str),
        Some("http://somewhere-else.example"),
        "a caller must not be able to redirect the agent's traffic"
    );
}

#[tokio::test]
async fn software_the_image_does_not_carry_is_refused_by_name() {
    // Nothing can be installed into a running sandbox, so accepting this would
    // start a session that quietly lacks the tools it was described as having.
    let a = adapter("http://127.0.0.1:1");
    let err = a
        .provision(&spec_with(crate::RuntimeEnvironment {
            packages: vec!["a-package-this-image-lacks".into()],
            ..Default::default()
        }))
        .await
        .expect_err("an undeliverable requirement must not provision");
    assert!(matches!(err, AdapterError::Unsupported(_)), "got {err:?}");
    assert!(
        err.to_string().contains("a-package-this-image-lacks"),
        "the refusal must name what is missing, got {err}"
    );
}

#[tokio::test]
async fn a_skill_the_agent_cannot_load_is_refused_by_name() {
    let a = adapter("http://127.0.0.1:1");
    let err = a
        .provision(&spec_with(crate::RuntimeEnvironment {
            skills: vec!["a-skill-this-agent-lacks".into()],
            ..Default::default()
        }))
        .await
        .expect_err("an undeliverable skill must not provision");
    assert!(matches!(err, AdapterError::Unsupported(_)), "got {err:?}");
    assert!(
        err.to_string().contains("a-skill-this-agent-lacks"),
        "{err}"
    );
}

#[tokio::test]
async fn what_the_image_already_carries_provisions_as_before() {
    // The success path, and the one that matters most: every session carries
    // this deployment's baseline, so a blanket refusal would have refused all
    // of them.
    let a = CodexAdapter::new(
        SandboxClient::new("http://127.0.0.1:1", "test-key"),
        CodexSandboxSpec {
            provided_packages: vec!["nyxid".into()],
            provided_skills: vec!["nyxid".into(), "ornn-agent-manual-cli".into()],
            ..CodexSandboxSpec::default()
        },
    );
    let err = a
        .provision(&spec_with(crate::RuntimeEnvironment {
            packages: vec!["nyxid".into()],
            skills: vec!["nyxid".into(), "ornn-agent-manual-cli".into()],
            ..Default::default()
        }))
        .await
        .expect_err("the dead provider is what fails, not the request");
    assert!(
        matches!(err, AdapterError::Unavailable(_)),
        "the baseline must reach the provider, got {err:?}"
    );
}

#[test]
fn the_runtime_is_told_where_this_kernel_keeps_its_conversation() {
    // The supervisor hosts any kernel and must not know this. Naming it here,
    // in the directory that owns this kernel, is what lets the same pod image
    // supervise a different one.
    let a = adapter("http://127.0.0.1:1");
    let env = a.environment_for(&spec_with(Default::default()));
    assert_eq!(
        env.get("CHRONO_RUNTIME_SNAPSHOT_DIR").map(String::as_str),
        Some(".codex/sessions")
    );
}

#[test]
fn a_terminal_link_asks_for_the_wire_it_speaks() {
    // Asked for rather than assumed. A pod that predates lanes ignores this
    // and keeps speaking what it always did, which is what makes a
    // half-rolled-out fleet safe.
    let a = adapter("http://provider:80");
    let url = a.terminal_url("provider/v1/sandboxes/sbx-1/proxy/9090", "sbx-1");
    assert!(
        url.ends_with(&format!(
            "/runtime/sbx-1/terminal?wire={}",
            supervisor::WIRE_VERSION
        )),
        "got {url}"
    );
}

#[tokio::test]
async fn a_runtime_that_declares_its_wire_gets_the_framed_dialect() {
    let mut said = futures_util::stream::iter(vec![Ok(Message::Text(
        r#"{"t":"ready","wireVersion":1,"lanes":["ctl","term"]}"#.into(),
    ))]);
    let (framed, early) = CodexAdapter::negotiate(&mut said).await;
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
    let (framed, early) = CodexAdapter::negotiate(&mut said).await;
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
    let (framed, early) = CodexAdapter::negotiate(&mut said).await;
    assert!(!framed);
    assert!(early.is_none());
}

#[tokio::test]
async fn an_unfamiliar_control_frame_is_not_read_as_a_declaration() {
    let mut said = futures_util::stream::iter(vec![Ok(Message::Text(r#"{"t":"pong"}"#.into()))]);
    let (framed, _) = CodexAdapter::negotiate(&mut said).await;
    assert!(
        !framed,
        "switching the wire on anything but a declaration would corrupt a pod that never agreed"
    );
}
