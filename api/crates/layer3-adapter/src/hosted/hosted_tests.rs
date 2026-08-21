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
use std::sync::Arc;

/// A hosted kernel talking to a provider at `base`.
fn hosted(base: &str) -> HostedKernel {
    HostedKernel::new(
        SandboxClient::new(base, "test-key"),
        SandboxShape::default(),
        "test-kernel",
        ".codex/skills",
        "AGENTS.md",
    )
}

/// A provisioning request for a session with nothing carried into it.
fn spec() -> RuntimeSpec {
    RuntimeSpec {
        restore_from: None,
        skills: Vec::new(),
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

#[tokio::test]
async fn a_conversations_control_lane_carries_a_credential() {
    // The defect this exists for: a credential reaches a runtime as a control
    // frame on a recorded link, and the only place that recorded one was
    // attaching a terminal. A conversation session attaches none — and two of
    // the three kernels serve nothing else — so such a session could never be
    // given its owner's credential and refused every call the agent made.
    let sent: Arc<tokio::sync::Mutex<Vec<Message>>> = Arc::default();
    let sink = Arc::new(tokio::sync::Mutex::new(Recorder(Arc::clone(&sent))));

    let control = super::control_lane(sink);
    control
        .send(TerminalInput::Credential("a-real-credential-value".into()))
        .await
        .expect("the lane accepts a credential");
    for _ in 0..50 {
        if !sent.lock().await.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }

    let frames = sent.lock().await.clone();
    assert_eq!(frames.len(), 1, "the credential must reach the socket");
    let Message::Text(body) = &frames[0] else {
        panic!("a credential travels as a control frame, not as bytes: {frames:?}");
    };
    let parsed: serde_json::Value = serde_json::from_str(body).expect("json");
    assert_eq!(parsed["t"], "credential");
    assert_eq!(parsed["value"], "a-real-credential-value");
}

#[tokio::test]
async fn a_conversations_control_lane_carries_nothing_else() {
    // Terminal bytes have no meaning on a socket nobody is painting, and the
    // conversation has its own typed channel. Anything else arriving here is
    // dropped rather than framed onto a lane it does not belong to.
    let sent: Arc<tokio::sync::Mutex<Vec<Message>>> = Arc::default();
    let sink = Arc::new(tokio::sync::Mutex::new(Recorder(Arc::clone(&sent))));

    let control = super::control_lane(sink);
    control
        .send(TerminalInput::Bytes(b"ls -la".to_vec()))
        .await
        .expect("accepted by the channel");
    tokio::time::sleep(std::time::Duration::from_millis(60)).await;

    assert!(
        sent.lock().await.is_empty(),
        "only a credential may travel this lane"
    );
}

/// A sink that keeps what was written, so a test can read the wire.
struct Recorder(Arc<tokio::sync::Mutex<Vec<Message>>>);

impl futures_util::Sink<Message> for Recorder {
    type Error = std::io::Error;

    fn poll_ready(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn start_send(self: std::pin::Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
        let kept = Arc::clone(&self.0);
        tokio::spawn(async move { kept.lock().await.push(item) });
        Ok(())
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn poll_close(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn a_runtime_that_never_answers_a_switch_is_not_recorded_as_having_switched() {
    // #289. A supervisor too old to know the frame ignores it in silence, so
    // "the frame reached the socket" is not evidence the program changed.
    // Waiting is what tells the two apart, and the error is what carries the
    // caller to a rotation — which is how such a session moved before the
    // in-place switch existed, and still moves correctly.
    let kernel = hosted("http://127.0.0.1:1");
    let (link, _held) = mpsc::channel::<TerminalInput>(4);
    kernel.remember_link("sbx_old", link).await;

    let started = std::time::Instant::now();
    let outcome = kernel
        .switch_plane(&RuntimeHandle("sbx_old".into()), "codex-acp")
        .await;

    assert!(
        matches!(outcome, Err(AdapterError::Unavailable(ref why)) if why.contains("did not answer")),
        "silence must be an error rather than a success: {outcome:?}"
    );
    assert!(
        started.elapsed() >= std::time::Duration::from_secs(1),
        "it waited for an answer rather than assuming one"
    );
}

#[tokio::test]
async fn a_switch_the_runtime_confirms_is_recorded_as_done() {
    // The positive case, and the only one that may be recorded in place.
    let kernel = hosted("http://127.0.0.1:1");
    let (link, mut held) = mpsc::channel::<TerminalInput>(4);
    kernel.remember_link("sbx_new", link).await;

    // Stand in for the pod: read the request off the link and answer it the
    // way a current supervisor does, on the wire.
    let waiters = kernel.switch_waiters();
    tokio::spawn(async move {
        if let Some(TerminalInput::SwitchPlane { command }) = held.recv().await {
            let frame =
                serde_json::json!({ "t": "switch_plane_result", "command": command, "accepted": true })
                    .to_string();
            let accepted = super::link::switch_answer(&frame).expect("the answer parses");
            waiters.settle("sbx_new", accepted).await;
        }
    });

    kernel
        .switch_plane(&RuntimeHandle("sbx_new".into()), "codex-acp")
        .await
        .expect("a confirmed switch succeeds");
}

#[tokio::test]
async fn a_refused_command_is_not_a_switch() {
    // The supervisor refuses anything outside its boot allowlist. That is a
    // negative answer, not a missing one, and it must not be recorded either.
    let kernel = hosted("http://127.0.0.1:1");
    let (link, mut held) = mpsc::channel::<TerminalInput>(4);
    kernel.remember_link("sbx_ref", link).await;

    let waiters = kernel.switch_waiters();
    tokio::spawn(async move {
        if held.recv().await.is_some() {
            waiters.settle("sbx_ref", false).await;
        }
    });

    let outcome = kernel
        .switch_plane(&RuntimeHandle("sbx_ref".into()), "rm")
        .await;
    assert!(
        matches!(outcome, Err(AdapterError::Unsupported(ref why)) if why.contains("refused")),
        "a refusal names itself: {outcome:?}"
    );
}

/// What a provider's answer means about whether a sandbox is still there.
///
/// The variant is not cosmetic: the core replaces a session's runtime on
/// `Unavailable` and leaves it alone on `Rejected`, so a status read into the
/// wrong one either destroys a working session or strands a dead one.
mod what_the_provider_said {
    use super::*;

    #[test]
    fn a_sandbox_the_provider_says_is_not_there_is_loss() {
        // The clearest statement of loss there is, and it used to arrive as a
        // refusal — the same variant as "answered and said no". Read that way,
        // a deleted sandbox would never be replaced and its session would sit
        // pointed at nothing forever.
        for status in [404_u16, 410] {
            let mapped = AdapterError::from(SandboxError::Rejected {
                status,
                detail: "no such sandbox".to_string(),
            });
            assert!(
                mapped.is_runtime_loss(),
                "{status} is the provider saying it is gone, got {mapped:?}"
            );
        }
    }

    #[test]
    fn a_provider_that_is_briefly_broken_is_not_a_lost_sandbox() {
        // 5xx is the provider failing to answer about anything, which says
        // nothing about this sandbox either way. Reading it as loss would
        // replace a healthy runtime because the provider had a bad minute.
        for status in [500_u16, 502, 503] {
            let mapped = AdapterError::from(SandboxError::Rejected {
                status,
                detail: "upstream".to_string(),
            });
            assert!(
                !mapped.is_runtime_loss(),
                "{status} says nothing about the sandbox, got {mapped:?}"
            );
        }
    }

    #[test]
    fn a_refusal_that_is_not_about_absence_stays_a_refusal() {
        let mapped = AdapterError::from(SandboxError::Rejected {
            status: 409,
            detail: "busy".to_string(),
        });
        assert!(
            matches!(mapped, AdapterError::Rejected(_)),
            "got {mapped:?}"
        );
    }

    #[test]
    fn a_provider_that_could_not_be_reached_is_loss() {
        let mapped =
            AdapterError::from(SandboxError::Unreachable("connection refused".to_string()));
        assert!(mapped.is_runtime_loss(), "got {mapped:?}");
    }

    #[test]
    fn a_stated_absence_is_neither_loss_nor_refusal() {
        // 501 means this image cannot do it, so another sandbox from the same
        // image would say the same. Replacing one is never the answer.
        let mapped = AdapterError::from(SandboxError::Rejected {
            status: 501,
            detail: "not implemented".to_string(),
        });
        assert!(
            matches!(mapped, AdapterError::Unsupported(_)),
            "got {mapped:?}"
        );
        assert!(!mapped.is_runtime_loss());
    }

    #[test]
    fn saying_it_with_more_context_does_not_change_what_was_said() {
        // Three call sites used to reword every failure as a refusal, so a
        // deadline and a closed link — both silence — reached the core as
        // though the runtime had answered.
        let loss = AdapterError::Unavailable("never answered".to_string())
            .context("could not resume conversation");
        assert!(loss.is_runtime_loss(), "got {loss:?}");
        assert!(format!("{loss}").contains("could not resume conversation"));

        let refused =
            AdapterError::Rejected("no".to_string()).context("the sign-in did not complete");
        assert!(!refused.is_runtime_loss(), "got {refused:?}");
        assert!(format!("{refused}").contains("the sign-in did not complete"));
    }
}

/// What a session is told about itself before it starts.
///
/// Written because nothing told it, and an agent that does not know it is
/// already authenticated goes looking for a way to log in — which the platform
/// CLI helpfully suggests, and which cannot work, because an agent key is not
/// a user. Observed on prod doing exactly that.
mod the_note_a_session_starts_with {
    use super::*;

    #[test]
    fn refuses_the_login_the_cli_would_otherwise_suggest() {
        let note = hosted("http://x").agent_note(&["ornn".to_string()]);
        assert!(
            note.contains("Do not run `nyxid login`"),
            "the instruction that stops the loop must be explicit: {note}"
        );
        assert!(
            note.contains("already authenticated"),
            "and it must say why there is nothing to log in as"
        );
    }

    #[test]
    fn names_every_service_the_session_may_reach() {
        let note =
            hosted("http://x").agent_note(&["ornn".to_string(), "api-github-pat".to_string()]);
        assert!(note.contains("`ornn`"), "got {note}");
        assert!(note.contains("`api-github-pat`"), "got {note}");
    }

    #[test]
    fn warns_that_discovery_lists_more_than_the_session_can_use() {
        // The trap this exists for: discovery answers with everything on the
        // account, so an agent picks a plausible slug and collects a refusal
        // it has no way to interpret.
        let note = hosted("http://x").agent_note(&["ornn".to_string()]);
        assert!(
            note.contains("proxy discover"),
            "the note must name the command that misleads: {note}"
        );
    }

    #[test]
    fn a_session_granted_nothing_is_told_so_rather_than_left_guessing() {
        // Distinct from "reaches these": a session with no services will have
        // every call refused, and saying so beats an empty list that reads
        // like the note was not filled in.
        let note = hosted("http://x").agent_note(&[]);
        assert!(note.contains("granted no services"), "got {note}");
        assert!(!note.contains("## What you can reach"), "got {note}");
    }
}
