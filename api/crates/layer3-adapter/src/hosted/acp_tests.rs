//! The handshake, against a bridge that answers.
//!
//! A real socket rather than a fake one, because what is being checked is the
//! ordering of a conversation with something that answers asynchronously: the
//! declaration is read first, the initialize is answered on the read loop that
//! was started before it was sent, and the mode — when there is one — is
//! settled before the caller is handed a channel it may prompt.

use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

use tokio::sync::mpsc;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use super::Bridge;
use crate::supervisor;
use crate::{AdapterError, RuntimeHandle};

fn hosted() -> super::super::HostedKernel {
    super::super::HostedKernel::new(
        crate::sandbox::SandboxClient::new("http://127.0.0.1:1", "test-key"),
        super::super::SandboxShape::default(),
        "test-kernel",
        ".codex/skills",
        "AGENTS.md",
    )
}

async fn wait_until_link_is_gone(kernel: &super::super::HostedKernel, handle: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while kernel.link_to(handle).await.is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the closed generation is promptly removed");
}

/// What the fake bridge did during one handshake.
struct Handshake {
    /// The JSON-RPC methods it was asked, in order.
    asked: Vec<String>,
    /// The mode it was told to hold the session to, if any.
    mode: Option<String>,
    /// Every frame it received, whole.
    ///
    /// Kept as well as the method list because a missing method is invisible
    /// in that list — it lands there as an empty string — and a missing method
    /// is exactly the defect that stopped every conversation turn: the prompt
    /// went out as a JSON-RPC request with an id and nothing to dispatch on.
    frames: Vec<serde_json::Value>,
}

/// Serve one bridge that answers everything, and hand back what it was asked.
///
/// `refuse` names a method the bridge answers with an error, which is how the
/// "a session whose mode did not take must not start" path is exercised.
/// `calls` is how many it should answer before returning what it saw — a
/// handshake that asks fewer would otherwise leave the fixture waiting for one
/// that is not coming.
async fn bridging(
    resuming: Option<&str>,
    refuse: Option<&'static str>,
    calls: Option<usize>,
) -> (String, tokio::task::JoinHandle<Handshake>) {
    bridging_offering(resuming, refuse, calls, serde_json::json!([])).await
}

/// The same fixture, with control over which sign-ins the bridge advertises.
async fn bridging_offering(
    resuming: Option<&str>,
    refuse: Option<&'static str>,
    calls: Option<usize>,
    auth_methods: serde_json::Value,
) -> (String, tokio::task::JoinHandle<Handshake>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a local port");
    let url = format!("ws://{}/", listener.local_addr().expect("an address"));
    let declared = declaration(resuming);

    let served = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.expect("a client");
        let mut ws = tokio_tungstenite::accept_async(socket)
            .await
            .expect("a websocket");
        ws.send(Message::Text(declared))
            .await
            .expect("the declaration is sent");

        let mut asked = Vec::new();
        let mut mode = None;
        let mut frames = Vec::new();
        while let Some(Ok(frame)) = ws.next().await {
            let Message::Binary(bytes) = frame else {
                continue;
            };
            let Some((supervisor::LANE_CONVERSATION, payload)) = supervisor::unframe(&bytes) else {
                continue;
            };
            let call: serde_json::Value = serde_json::from_slice(payload).expect("json-rpc");
            frames.push(call.clone());
            let method = call["method"].as_str().unwrap_or_default().to_string();
            if method == "session/set_mode" {
                mode = call["params"]["modeId"].as_str().map(str::to_string);
            }
            asked.push(method.clone());
            let answer = match refuse {
                Some(refused) if refused == method => serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": call["id"],
                    "error": { "code": -32000, "message": "no" },
                }),
                // `initialize` answers with the sign-ins this bridge offers,
                // because that is what an own-account session chooses from.
                // Every other call keeps the one-size answer: these tests are
                // about which calls are made and in what order.
                _ if method == "initialize" => serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": call["id"],
                    "result": {
                        "sessionId": "conv-1",
                        "authMethods": auth_methods.clone(),
                    },
                }),
                _ => serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": call["id"],
                    "result": { "sessionId": "conv-1" },
                }),
            };
            let body = answer.to_string();
            let framed = supervisor::frame(supervisor::LANE_CONVERSATION, body.as_bytes());
            if ws.send(Message::Binary(framed)).await.is_err() {
                break;
            }
            if let Some(expected) = calls {
                if asked.len() == expected {
                    break;
                }
            }
        }
        Handshake {
            asked,
            mode,
            frames,
        }
    });

    (url, served)
}

async fn bridge_holding_initialize() -> (
    String,
    tokio::sync::oneshot::Receiver<()>,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a local port");
    let url = format!("ws://{}/", listener.local_addr().expect("an address"));
    let (started, started_rx) = tokio::sync::oneshot::channel();
    let served = tokio::spawn(async move {
        let mut started = Some(started);
        let (socket, _) = listener.accept().await.expect("a client");
        let mut ws = tokio_tungstenite::accept_async(socket)
            .await
            .expect("a websocket");
        ws.send(Message::Text(declaration(None)))
            .await
            .expect("the declaration is sent");
        while let Some(frame) = ws.next().await {
            match frame {
                Ok(Message::Binary(bytes))
                    if supervisor::unframe(&bytes)
                        .is_some_and(|(lane, _)| lane == supervisor::LANE_CONVERSATION) =>
                {
                    if let Some(started) = started.take() {
                        let _ = started.send(());
                    }
                    break;
                }
                Ok(_) => {}
                Err(_) => return,
            }
        }
        while let Some(frame) = ws.next().await {
            match frame {
                Ok(Message::Close(_)) | Err(_) => break,
                Ok(_) => {}
            }
        }
    });
    (url, started_rx, served)
}

/// A bridge that asks its owner to sign in, and waits there.
///
/// The other fixtures answer `authenticate` immediately, which is the one
/// thing a real device sign-in never does — it is a person walking to another
/// machine. This one reproduces the wait: it sends `elicitation/create` and
/// holds the `authenticate` answer until the client has replied, so anything
/// the client is expected to do *during* the sign-in is observable.
async fn bridging_that_waits_for_a_sign_in() -> (String, tokio::task::JoinHandle<Handshake>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a local port");
    let url = format!("ws://{}/", listener.local_addr().expect("an address"));
    let declared = declaration(None);

    let served = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.expect("a client");
        let mut ws = tokio_tungstenite::accept_async(socket)
            .await
            .expect("a websocket");
        ws.send(Message::Text(declared))
            .await
            .expect("the declaration is sent");

        let mut asked = Vec::new();
        let mut frames = Vec::new();
        let mut authenticate_id = None;
        async fn send(
            ws: &mut WebSocketStream<tokio::net::TcpStream>,
            value: serde_json::Value,
        ) -> bool {
            let body = value.to_string();
            let framed = supervisor::frame(supervisor::LANE_CONVERSATION, body.as_bytes());
            ws.send(Message::Binary(framed)).await.is_ok()
        }

        while let Some(Ok(frame)) = ws.next().await {
            let Message::Binary(bytes) = frame else {
                continue;
            };
            let Some((supervisor::LANE_CONVERSATION, payload)) = supervisor::unframe(&bytes) else {
                continue;
            };
            let call: serde_json::Value = serde_json::from_slice(payload).expect("json-rpc");
            frames.push(call.clone());

            // The client's answer to the sign-in. Only now does the kernel
            // consider itself authenticated, which is the whole point.
            if call.get("method").is_none() {
                if let Some(id) = authenticate_id.take() {
                    if !send(
                        &mut ws,
                        serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": {} }),
                    )
                    .await
                    {
                        break;
                    }
                }
                continue;
            }

            let method = call["method"].as_str().unwrap_or_default().to_string();
            asked.push(method.clone());

            if method == "authenticate" {
                authenticate_id = Some(call["id"].clone());
                let elicit = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 9001,
                    "method": "elicitation/create",
                    "params": {
                        "message": "Sign in to continue",
                        "url": "https://auth.example.invalid/device",
                        "userCode": "ABCD-1234",
                    },
                });
                if !send(&mut ws, elicit).await {
                    break;
                }
                continue;
            }

            let answer = match method.as_str() {
                "initialize" => serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": call["id"],
                    "result": { "sessionId": "conv-1", "authMethods": device_code_offer() },
                }),
                _ => serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": call["id"],
                    "result": { "sessionId": "conv-1" },
                }),
            };
            if !send(&mut ws, answer).await {
                break;
            }
            if asked.len() == 3 {
                break;
            }
        }
        Handshake {
            asked,
            mode: None,
            frames,
        }
    });

    (url, served)
}

/// The declaration a pod serving a conversation lane opens with.
fn declaration(resuming: Option<&str>) -> String {
    serde_json::json!({
        "t": "ready",
        "wireVersion": supervisor::WIRE_VERSION,
        "lanes": ["ctl", "term", "conv"],
        "resuming": resuming,
    })
    .to_string()
}

/// Connect to a fixture bridge the way the adapter connects to a real one.
async fn connect(
    url: &str,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let (stream, _) = tokio_tungstenite::connect_async(url)
        .await
        .expect("the fixture accepts");
    stream
}

/// Send one framed JSON-RPC value from a fixture bridge.
async fn send_rpc(ws: &mut WebSocketStream<tokio::net::TcpStream>, value: serde_json::Value) {
    let body = value.to_string();
    let framed = supervisor::frame(supervisor::LANE_CONVERSATION, body.as_bytes());
    ws.send(Message::Binary(framed)).await.expect("frame sent");
}

/// Receive one framed JSON-RPC value at a fixture bridge.
async fn receive_rpc(ws: &mut WebSocketStream<tokio::net::TcpStream>) -> serde_json::Value {
    loop {
        let frame = ws.next().await.expect("a frame").expect("a valid frame");
        let Message::Binary(bytes) = frame else {
            continue;
        };
        let Some((supervisor::LANE_CONVERSATION, payload)) = supervisor::unframe(&bytes) else {
            continue;
        };
        return serde_json::from_slice(payload).expect("JSON-RPC");
    }
}

/// Open a bridge the way the core does: already draining before it calls.
///
/// The receiver comes back beside the channel so a test can assert on what was
/// reported *during* the open — which is the only place a sign-in request
/// exists, since the kernel is blocked on it for the whole of the rest of it.
async fn open_bridge(
    stream: WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    cwd: String,
    mode: Option<String>,
    rejoin: Option<String>,
    link: Option<super::super::LinkLifecycle>,
    own_credential: bool,
) -> Result<
    (
        crate::ConversationChannel,
        crate::TerminalInputSender,
        mpsc::Receiver<crate::ConversationUpdate>,
    ),
    AdapterError,
> {
    let (updates_tx, updates_rx) = mpsc::channel(64);
    let (channel, control) =
        Bridge::open(stream, cwd, mode, rejoin, link, own_credential, updates_tx).await?;
    Ok((channel, control, updates_rx))
}

#[tokio::test]
async fn a_close_as_open_finishes_cannot_register_a_dead_generation() {
    // The fixture closes immediately after answering the two-call handshake.
    // Previously the reader cleaned up first and the caller then inserted its
    // dead control sender after `open` returned.
    let kernel = hosted();
    let handle = RuntimeHandle("sbx_close_during_open".into());
    let (url, served) = bridging(None, None, Some(2)).await;
    let stream = connect(&url).await;
    let opened = open_bridge(
        stream,
        "/home/chrono".into(),
        None,
        None,
        Some(kernel.link_lifecycle(&handle.0)),
        false,
    )
    .await;
    match opened {
        Ok((_channel, _control, _updates)) => {}
        Err(AdapterError::Unavailable(reason))
            if reason == "the conversation link ended or was superseded while opening" => {}
        Err(error) => panic!("unexpected handshake-boundary error: {error}"),
    }
    served
        .await
        .expect("the fixture closes after the handshake");
    wait_until_link_is_gone(&kernel, &handle.0).await;

    assert!(
        kernel.refresh_credential(&handle, "unused").await.is_err(),
        "a closed queue must not report credential delivery"
    );
    let switched = tokio::time::timeout(
        std::time::Duration::from_millis(250),
        kernel.switch_plane(&handle, "codex-acp"),
    )
    .await
    .expect("a closed generation is rejected without waiting for the switch deadline");
    assert!(
        matches!(switched, Err(AdapterError::Unavailable(ref why)) if why.contains("no open link")),
        "the closed generation is absent: {switched:?}"
    );
}

#[tokio::test]
async fn a_conversation_close_removes_its_registered_generation() {
    let kernel = hosted();
    let handle = RuntimeHandle("sbx_close_after_open".into());
    let (url, served) = bridging(None, None, None).await;
    let stream = connect(&url).await;
    let (channel, control, _updates) = open_bridge(
        stream,
        "/home/chrono".into(),
        None,
        None,
        Some(kernel.link_lifecycle(&handle.0)),
        false,
    )
    .await
    .expect("the live bridge opens");

    let registered = kernel
        .link_to(&handle.0)
        .await
        .expect("the bridge registers before returning");
    assert!(registered.same_channel(&control));

    served.abort();
    let _ = served.await;
    wait_until_link_is_gone(&kernel, &handle.0).await;
    assert!(channel
        .input
        .send(crate::ConversationInput::Cancel)
        .await
        .is_err());
    assert!(control
        .send(crate::TerminalInput::Credential {
            operation_id: "unused-operation".into(),
            value: "unused".into(),
        })
        .await
        .is_err());
}

#[tokio::test]
async fn a_fresh_conversation_is_started_and_held_to_its_mode() {
    let (url, served) = bridging(None, None, Some(3)).await;
    let stream = connect(&url).await;
    open_bridge(
        stream,
        "/home/chrono".into(),
        Some("read-only".into()),
        None,
        None,
        false,
    )
    .await
    .expect("the bridge answered everything");
    let done = served.await.expect("the fixture finished");
    assert_eq!(
        done.asked,
        vec!["initialize", "session/new", "session/set_mode"],
        "a session is initialized, started, then pinned to what its owner asked for"
    );
    assert_eq!(done.mode.as_deref(), Some("read-only"));
}

#[tokio::test]
async fn a_carried_conversation_is_loaded_rather_than_started_again() {
    // The §5 half: a pod that was handed a conversation resumes it by
    // identifier, so its owner's history is still there after a rotation.
    let (url, served) = bridging(Some("conv-carried"), None, Some(3)).await;
    let stream = connect(&url).await;
    open_bridge(
        stream,
        "/home/chrono".into(),
        Some("agent".into()),
        None,
        None,
        false,
    )
    .await
    .expect("the bridge answered everything");
    let done = served.await.expect("the fixture finished");
    assert_eq!(
        done.asked,
        vec!["initialize", "session/load", "session/set_mode"],
        "a carried conversation must be loaded, never started fresh"
    );
}

#[tokio::test]
async fn a_bridge_with_no_modes_is_not_asked_to_set_one() {
    // Not every kernel's bridge has modes. Sending one anyway would fail the
    // handshake against a bridge that is behaving correctly, so the kernel
    // names none and this step is skipped rather than defaulted.
    let (url, served) = bridging(None, None, Some(3)).await;
    let stream = connect(&url).await;
    open_bridge(stream, "/home/chrono".into(), None, None, None, false)
        .await
        .expect("a bridge without modes still opens");
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    served.abort();
}

#[tokio::test]
async fn a_session_whose_mode_did_not_take_is_refused_rather_than_started() {
    // The failure this refusal exists for: bridges start in their most
    // permissive mode, so a session that silently kept it would be acting
    // unattended while its owner had asked to be asked.
    let (url, served) = bridging(None, Some("session/set_mode"), Some(3)).await;
    let stream = connect(&url).await;
    let err = open_bridge(
        stream,
        "/home/chrono".into(),
        Some("read-only".into()),
        None,
        None,
        false,
    )
    .await
    .expect_err("a mode that did not take must fail the session");
    assert!(
        err.to_string().contains("read-only"),
        "the refusal must name the mode that was not honoured, got {err}"
    );
    served.abort();
}

#[tokio::test]
async fn a_failed_handshake_closes_a_runtime_that_keeps_its_socket_open() {
    let kernel = hosted();
    let handle = RuntimeHandle("sbx_rejected_open".into());
    // No call limit: after refusing the mode, the fixture deliberately waits
    // for another frame and keeps the WebSocket open until the client leaves.
    let (url, served) = bridging(None, Some("session/set_mode"), None).await;
    let stream = connect(&url).await;
    let error = open_bridge(
        stream,
        "/home/chrono".into(),
        Some("read-only".into()),
        None,
        Some(kernel.link_lifecycle(&handle.0)),
        false,
    )
    .await
    .expect_err("a rejected mode fails the handshake");
    assert!(matches!(error, AdapterError::Rejected(_)), "got {error:?}");

    let observed = tokio::time::timeout(std::time::Duration::from_secs(1), served)
        .await
        .expect("the failed open closes its held runtime socket")
        .expect("the fixture does not panic");
    assert_eq!(
        observed.asked,
        vec!["initialize", "session/new", "session/set_mode"]
    );
    wait_until_link_is_gone(&kernel, &handle.0).await;
}

#[tokio::test]
async fn canceling_open_during_a_held_handshake_retires_the_whole_generation() {
    let kernel = hosted();
    let handle = RuntimeHandle("sbx_canceled_acp_open".into());
    let (url, initialize_started, served) = bridge_holding_initialize().await;
    let stream = connect(&url).await;
    let (updates, _received) = mpsc::channel(8);
    let opening = tokio::spawn(Bridge::open(
        stream,
        "/home/chrono".into(),
        None,
        None,
        Some(kernel.link_lifecycle(&handle.0)),
        false,
        updates,
    ));

    initialize_started
        .await
        .expect("the open reaches its held initialize call");
    assert!(
        kernel.link_to(&handle.0).await.is_some(),
        "the generation is registered while its handshake is in flight"
    );
    opening.abort();
    assert!(opening
        .await
        .expect_err("the open future is canceled")
        .is_cancelled());

    tokio::time::timeout(std::time::Duration::from_secs(1), served)
        .await
        .expect("canceling open disconnects the held runtime peer")
        .expect("the held bridge fixture does not panic");
    wait_until_link_is_gone(&kernel, &handle.0).await;
}

#[tokio::test]
async fn a_conversation_that_could_not_be_resumed_fails_loudly() {
    // The runtime this came from is about to be torn down. Reporting success
    // here would lose somebody's history with nothing said about it.
    let (url, served) = bridging(Some("conv-carried"), Some("session/load"), Some(3)).await;
    let stream = connect(&url).await;
    let err = open_bridge(
        stream,
        "/home/chrono".into(),
        Some("agent".into()),
        None,
        None,
        false,
    )
    .await
    .expect_err("a conversation that did not resume must not look resumed");
    assert!(
        matches!(err, crate::AdapterError::Rejected(_)),
        "got {err:?}"
    );
    served.abort();
}

#[tokio::test]
async fn a_link_that_declares_nothing_is_not_a_conversation() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a local port");
    let url = format!("ws://{}/", listener.local_addr().expect("an address"));
    let served = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.expect("a client");
        let mut ws = tokio_tungstenite::accept_async(socket)
            .await
            .expect("a websocket");
        // Says nothing, then goes away.
        let _ = ws.close(None).await;
    });
    let stream = connect(&url).await;
    let err = open_bridge(stream, "/home/chrono".into(), None, None, None, false)
        .await
        .expect_err("a silent link is not a conversation");
    assert!(
        matches!(err, crate::AdapterError::Unavailable(_)),
        "got {err:?}"
    );
    served.abort();
}

#[tokio::test]
async fn a_conversation_the_bridge_still_holds_is_adopted_rather_than_loaded() {
    // The case a control plane restart produces, and the reason it is not the
    // same as a rotation: the bridge in the pod kept running and holds this
    // session in memory. Asking it to load one would ask it to read a file it
    // may not have written — a conversation with no committed turns writes
    // nothing — and a real bridge answers "session not found".
    //
    // So nothing is asked of it beyond the identifier, and the only call after
    // initialize is the mode.
    let (url, served) = bridging(None, None, Some(2)).await;
    let stream = connect(&url).await;
    open_bridge(
        stream,
        "/home/chrono".into(),
        Some("read-only".into()),
        Some("conv-already-running".into()),
        None,
        false,
    )
    .await
    .expect("a held conversation is adopted");
    let done = served.await.expect("the fixture finished");
    assert_eq!(
        done.asked,
        vec!["initialize", "session/set_mode"],
        "neither a new session nor a load: it is already there"
    );
}

#[tokio::test]
async fn a_bridge_that_was_already_greeted_is_taken_over_rather_than_abandoned() {
    /*
     * #416, reproduced. `initialize` is once per bridge, not once per
     * connection, and a bot pod outlives the control plane that started it.
     * So a control plane taking back over a live runtime meets a kernel its
     * predecessor already greeted, and the kernel says so.
     *
     * In production that refusal was read as the sandbox being gone, and a
     * fresh one was provisioned over the top of a runtime that was still
     * working — the session came back empty beside the pod still holding its
     * conversation, on every rollout.
     *
     * The refusal only exists because something was there to send it, so the
     * handshake carries on. The mode still follows, which is what proves the
     * takeover completed rather than merely not erroring.
     */
    let (url, served) = bridging(None, Some("initialize"), Some(2)).await;
    let stream = connect(&url).await;
    open_bridge(
        stream,
        "/home/chrono".into(),
        Some("read-only".into()),
        Some("conv-already-running".into()),
        None,
        false,
    )
    .await
    .expect("a bridge that refuses a second greeting is still a live bridge");
    let done = served.await.expect("the fixture finished");
    assert_eq!(
        done.asked,
        vec!["initialize", "session/set_mode"],
        "the greeting was refused and the takeover continued anyway"
    );
}

#[tokio::test]
async fn a_bridge_that_answers_nothing_is_not_mistaken_for_one_that_refused() {
    /*
     * The other half, and the one that keeps genuine loss recoverable.
     *
     * A refusal is tolerated because a frame came back. Silence must not be:
     * a runtime that has gone away answers nothing, and reading that as "it
     * refused, carry on" would leave a session pointed at a sandbox that no
     * longer exists and never replace it.
     *
     * The link is closed under the waiting call, which is what a vanished
     * runtime looks like from here.
     */
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a local port");
    let url = format!("ws://{}/", listener.local_addr().expect("an address"));
    let declared = declaration(None);
    tokio::spawn(async move {
        let (socket, _) = listener.accept().await.expect("a client");
        let mut ws = tokio_tungstenite::accept_async(socket)
            .await
            .expect("a websocket");
        ws.send(Message::Text(declared))
            .await
            .expect("the declaration is sent");
        // Take the greeting and then vanish, answering nothing.
        let _ = ws.next().await;
        drop(ws);
    });
    let stream = connect(&url).await;
    let failed = open_bridge(
        stream,
        "/home/chrono".into(),
        Some("read-only".into()),
        Some("conv-already-running".into()),
        None,
        false,
    )
    .await
    .expect_err("silence is not a refusal");
    assert!(
        failed.is_runtime_loss(),
        "a runtime that answered nothing must stay recoverable as loss, got {failed:?}"
    );
}

#[tokio::test]
async fn rejoining_wins_over_what_the_pod_says_it_was_handed() {
    // Both can be present: a pod that took a conversation over, whose control
    // plane then restarted. The record is the more recent truth, and it names
    // the conversation that is running now.
    let (url, served) = bridging(Some("conv-carried"), None, Some(2)).await;
    let stream = connect(&url).await;
    open_bridge(
        stream,
        "/home/chrono".into(),
        Some("agent".into()),
        Some("conv-already-running".into()),
        None,
        false,
    )
    .await
    .expect("adopted");
    let done = served.await.expect("the fixture finished");
    assert!(
        !done.asked.iter().any(|m| m == "session/load"),
        "a held conversation must not be loaded from disk, got {:?}",
        done.asked
    );
}

#[tokio::test]
async fn a_prompt_goes_out_as_a_dispatchable_json_rpc_request() {
    // The defect this exists for made every conversation turn hang, on every
    // kernel that speaks this protocol. The prompt was built as its parameters
    // alone and handed to the sender, which added only `jsonrpc` and `id` — so
    // what went on the wire was a request with no `method` and its parameters
    // at the top level. Nothing could dispatch it, the agent was never asked
    // anything, and the turn sat open until somebody gave up.
    //
    // Asserted on the serialised frame rather than on the call, because that
    // is where it was wrong. Every existing test here passed throughout.
    let (url, served) = bridging(None, None, Some(3)).await;
    let stream = connect(&url).await;
    let (channel, _control, _updates) =
        open_bridge(stream, "/home/chrono".into(), None, None, None, false)
            .await
            .expect("the bridge answered the handshake");

    channel
        .input
        .send(crate::ConversationInput::Prompt(vec![
            crate::ContentBlock::from("hi"),
        ]))
        .await
        .expect("the bridge is listening");

    let done = served.await.expect("the fixture finished");
    let prompt = done
        .frames
        .iter()
        .find(|f| f["method"] == "session/prompt")
        .unwrap_or_else(|| {
            panic!(
                "no session/prompt was ever sent; the bridge saw {:?}",
                done.frames
            )
        });

    assert_eq!(prompt["jsonrpc"], "2.0");
    assert!(
        prompt["id"].is_number(),
        "an answer has to be routable back"
    );
    assert!(
        prompt["params"]["sessionId"].is_string(),
        "the parameters belong under `params`, not at the top level: {prompt}"
    );
    assert!(prompt["params"]["prompt"].is_array(), "{prompt}");
}

#[tokio::test]
async fn an_interrupt_is_accepted_after_it_reaches_the_runtime_link() {
    let (url, served) = bridging(None, None, Some(3)).await;
    let stream = connect(&url).await;
    let (channel, _control, _updates) =
        open_bridge(stream, "/home/chrono".into(), None, None, None, false)
            .await
            .expect("the bridge answered the handshake");

    channel
        .input
        .send(crate::ConversationInput::Cancel)
        .await
        .expect("the interrupt reached the bridge link");

    let done = served.await.expect("the fixture finished");
    assert!(
        done.frames
            .iter()
            .any(|frame| frame["method"] == "session/cancel"),
        "the bridge must receive session/cancel before the caller sees success: {:?}",
        done.frames
    );
}

#[tokio::test]
async fn every_request_this_bridge_sends_is_dispatchable() {
    // The general form, so the next request added cannot repeat it. A JSON-RPC
    // request without a method is undispatchable whatever it was meant to ask,
    // and the failure is silent at both ends.
    // Three handshake calls plus the prompt this test inspects. Stopping the
    // fixture after three closes the runtime link before the prompt and is
    // now correctly refused rather than accepted into a dead local queue.
    let (url, served) = bridging(None, None, Some(4)).await;
    let stream = connect(&url).await;
    let (channel, _control, _updates) = open_bridge(
        stream,
        "/home/chrono".into(),
        Some("agent".into()),
        None,
        None,
        false,
    )
    .await
    .expect("the bridge answered the handshake");
    channel
        .input
        .send(crate::ConversationInput::Prompt(vec![
            crate::ContentBlock::from("anything"),
        ]))
        .await
        .expect("the bridge is listening");

    let done = served.await.expect("the fixture finished");
    for frame in &done.frames {
        // A response carries a result rather than a method; only requests and
        // notifications are being checked here.
        if frame.get("result").is_some() || frame.get("error").is_some() {
            continue;
        }
        assert!(
            frame["method"].is_string(),
            "a request with no method cannot be dispatched: {frame}"
        );
        assert_eq!(frame["jsonrpc"], "2.0", "{frame}");
    }
}

#[tokio::test]
async fn a_permission_answer_survives_replacing_the_entire_hosted_bridge() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a local port");
    let url = format!("ws://{}/", listener.local_addr().expect("an address"));
    let call_id = serde_json::json!("permission-call-before-restart");
    let request = crate::RequestPermissionRequest::new(
        crate::ConversationId::new("conv-durable-permission"),
        crate::ToolCallUpdate::new(
            crate::ToolCallId::new("tool-call-not-in-locator"),
            crate::ToolCallUpdateFields::new().title("sensitive tool title".to_string()),
        ),
        vec![
            crate::PermissionOption::new(
                crate::PermissionOptionId::new("reject-once"),
                "Reject this time".to_string(),
                crate::PermissionOptionKind::RejectOnce,
            ),
            crate::PermissionOption::new(
                crate::PermissionOptionId::new("allow-once"),
                "Allow this time".to_string(),
                crate::PermissionOptionKind::AllowOnce,
            ),
        ],
    );
    let request_params = serde_json::to_value(request).expect("the ACP request serializes");
    let expected_call_id = call_id.clone();

    let served = tokio::spawn(async move {
        // The original control plane opens the conversation and receives the
        // request during a real prompt. Closing this whole socket discards both
        // its permission route and the waiter for that prompt's response.
        let (socket, _) = listener.accept().await.expect("the first client");
        let mut first = tokio_tungstenite::accept_async(socket)
            .await
            .expect("a websocket");
        first
            .send(Message::Text(declaration(None)))
            .await
            .expect("the declaration is sent");
        let prompt_call = loop {
            let call = receive_rpc(&mut first).await;
            let method = call["method"].as_str().unwrap_or_default();
            if method == "session/prompt" {
                send_rpc(
                    &mut first,
                    serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": expected_call_id,
                        "method": "session/request_permission",
                        "params": request_params.clone(),
                    }),
                )
                .await;
                let prompt_call = call["id"].clone();
                first.close(None).await.expect("the old link closes");
                break prompt_call;
            }
            let result = if method == "session/new" {
                serde_json::json!({ "sessionId": "conv-durable-permission" })
            } else {
                serde_json::json!({})
            };
            send_rpc(
                &mut first,
                serde_json::json!({ "jsonrpc": "2.0", "id": call["id"], "result": result }),
            )
            .await;
        };

        // A replacement control plane gets a new Bridge with an empty blocked
        // and prompt-waiter maps, rejoins the held conversation, and returns the
        // old permission call's response from the durable locator alone.
        let (socket, _) = listener.accept().await.expect("the replacement client");
        let mut replacement = tokio_tungstenite::accept_async(socket)
            .await
            .expect("a websocket");
        replacement
            .send(Message::Text(declaration(None)))
            .await
            .expect("the declaration is sent");
        let initialize = receive_rpc(&mut replacement).await;
        assert_eq!(initialize["method"], "initialize");
        send_rpc(
            &mut replacement,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": initialize["id"],
                "result": {},
            }),
        )
        .await;
        // The runtime may replay the still-blocked request onto the replacement
        // link. That repopulates `blocked`, but it does not recreate the old
        // prompt waiter; its eventual response must still use the orphan path.
        send_rpc(
            &mut replacement,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": expected_call_id,
                "method": "session/request_permission",
                "params": request_params,
            }),
        )
        .await;
        let permission_answer = receive_rpc(&mut replacement).await;
        send_rpc(
            &mut replacement,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": prompt_call,
                "result": { "stopReason": "end_turn" },
            }),
        )
        .await;
        permission_answer
    });

    let first_stream = connect(&url).await;
    let (first_channel, first_control, mut first_updates) =
        open_bridge(first_stream, "/home/chrono".into(), None, None, None, false)
            .await
            .expect("the original bridge opens");
    first_channel
        .input
        .send(crate::ConversationInput::Prompt(vec![
            crate::ContentBlock::from("run something guarded"),
        ]))
        .await
        .expect("the original prompt reaches the bridge");
    let asked = tokio::time::timeout(std::time::Duration::from_secs(1), first_updates.recv())
        .await
        .expect("the request arrives")
        .expect("the update stream stays open");
    let crate::ConversationUpdate::Asked(asked) = asked else {
        panic!("expected a permission request, got {asked:?}");
    };
    let locator = asked
        .locator
        .expect("hosted requests carry a durable locator");
    assert!(
        !locator.as_str().contains("sensitive tool title")
            && !locator.as_str().contains("tool-call-not-in-locator")
            && !locator.as_str().contains("Allow this time"),
        "the locator contains routing metadata, not the tool payload or option labels"
    );
    drop(first_channel);
    drop(first_control);
    drop(first_updates);

    let replacement_stream = connect(&url).await;
    let (replacement, _control, mut replacement_updates) = open_bridge(
        replacement_stream,
        "/home/chrono".into(),
        None,
        Some("conv-durable-permission".into()),
        None,
        false,
    )
    .await
    .expect("the replacement bridge rejoins");
    let replayed = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        replacement_updates.recv(),
    )
    .await
    .expect("the runtime replays its blocked request")
    .expect("the replacement update stream stays open");
    let crate::ConversationUpdate::Asked(replayed) = replayed else {
        panic!("expected the replayed permission request, got {replayed:?}");
    };
    assert_eq!(replayed.id, asked.id);
    replacement
        .input
        .send(crate::ConversationInput::PermissionAnswer {
            id: asked.id,
            locator: Some(locator),
            decision: crate::PermissionDecision::Allow,
        })
        .await
        .expect("the restored answer reaches the hosted bridge");

    let completed = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        replacement_updates.recv(),
    )
    .await
    .expect("the old prompt's terminal response is not dropped")
    .expect("the replacement update stream stays open");
    assert!(
        matches!(completed, crate::ConversationUpdate::TurnEnded(_)),
        "the replacement must close the original turn, got {completed:?}"
    );

    let answer = served.await.expect("the fixture completes");
    assert_eq!(
        answer["id"], call_id,
        "the original JSON-RPC call is answered"
    );
    assert_eq!(answer["result"]["outcome"]["outcome"], "selected");
    assert_eq!(
        answer["result"]["outcome"]["optionId"], "allow-once",
        "the answer selects an option the original call genuinely offered"
    );
}

#[tokio::test]
async fn a_reused_json_rpc_permission_id_is_new_in_a_later_turn() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a local port");
    let url = format!("ws://{}/", listener.local_addr().expect("an address"));

    let served = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.expect("a client");
        let mut ws = tokio_tungstenite::accept_async(socket)
            .await
            .expect("a websocket");
        ws.send(Message::Text(declaration(None)))
            .await
            .expect("the declaration is sent");

        let first_prompt = loop {
            let call = receive_rpc(&mut ws).await;
            let method = call["method"].as_str().unwrap_or_default();
            if method == "session/prompt" {
                break call["id"].clone();
            }
            let result = if method == "session/new" {
                serde_json::json!({ "sessionId": "conv-reused-permission-id" })
            } else {
                serde_json::json!({})
            };
            send_rpc(
                &mut ws,
                serde_json::json!({ "jsonrpc": "2.0", "id": call["id"], "result": result }),
            )
            .await;
        };

        let options = || {
            vec![
                crate::PermissionOption::new(
                    crate::PermissionOptionId::new("allow-once"),
                    "Allow once".to_string(),
                    crate::PermissionOptionKind::AllowOnce,
                ),
                crate::PermissionOption::new(
                    crate::PermissionOptionId::new("reject-once"),
                    "Reject once".to_string(),
                    crate::PermissionOptionKind::RejectOnce,
                ),
            ]
        };
        let request = |tool: &str, title: &str| {
            crate::RequestPermissionRequest::new(
                crate::ConversationId::new("conv-reused-permission-id"),
                crate::ToolCallUpdate::new(
                    crate::ToolCallId::new(tool),
                    crate::ToolCallUpdateFields::new().title(title.to_string()),
                ),
                options(),
            )
        };

        send_rpc(
            &mut ws,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": "reused-call-id",
                "method": "session/request_permission",
                "params": request("first-tool", "first action"),
            }),
        )
        .await;
        let first_answer = receive_rpc(&mut ws).await;
        send_rpc(
            &mut ws,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": first_prompt,
                "result": { "stopReason": "end_turn" },
            }),
        )
        .await;

        let second_prompt = receive_rpc(&mut ws).await;
        assert_eq!(second_prompt["method"], "session/prompt");
        send_rpc(
            &mut ws,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": "reused-call-id",
                "method": "session/request_permission",
                "params": request("second-tool", "second action"),
            }),
        )
        .await;
        let second_answer = receive_rpc(&mut ws).await;
        send_rpc(
            &mut ws,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": second_prompt["id"],
                "result": { "stopReason": "end_turn" },
            }),
        )
        .await;
        (first_answer, second_answer)
    });

    let stream = connect(&url).await;
    let (channel, _control, mut updates) =
        open_bridge(stream, "/home/chrono".into(), None, None, None, false)
            .await
            .expect("the bridge opens");
    channel
        .input
        .send(crate::ConversationInput::Prompt(vec![
            crate::ContentBlock::from("first turn"),
        ]))
        .await
        .expect("the first prompt is sent");
    let first = tokio::time::timeout(std::time::Duration::from_secs(1), updates.recv())
        .await
        .expect("the first permission arrives")
        .expect("the update stream stays open");
    let crate::ConversationUpdate::Asked(first) = first else {
        panic!("expected the first permission request, got {first:?}");
    };
    let public_id = first.id.clone();
    channel
        .input
        .send(crate::ConversationInput::PermissionAnswer {
            id: first.id,
            locator: first.locator,
            decision: crate::PermissionDecision::Allow,
        })
        .await
        .expect("the first answer is sent");
    let first_ending = tokio::time::timeout(std::time::Duration::from_secs(1), updates.recv())
        .await
        .expect("the first turn ends")
        .expect("the update stream stays open");
    assert!(matches!(
        first_ending,
        crate::ConversationUpdate::TurnEnded(_)
    ));

    channel
        .input
        .send(crate::ConversationInput::Prompt(vec![
            crate::ContentBlock::from("second turn"),
        ]))
        .await
        .expect("the second prompt is sent");
    let second = tokio::time::timeout(std::time::Duration::from_secs(1), updates.recv())
        .await
        .expect("a reused JSON-RPC id is surfaced as a new request")
        .expect("the update stream stays open");
    let crate::ConversationUpdate::Asked(second) = second else {
        panic!("expected the second permission request, got {second:?}");
    };
    assert_eq!(
        second.id, public_id,
        "the wire id and normalized options were deliberately reused"
    );
    channel
        .input
        .send(crate::ConversationInput::PermissionAnswer {
            id: second.id,
            locator: second.locator,
            decision: crate::PermissionDecision::Deny,
        })
        .await
        .expect("the second request accepts its own decision");
    let second_ending = tokio::time::timeout(std::time::Duration::from_secs(1), updates.recv())
        .await
        .expect("the second turn ends")
        .expect("the update stream stays open");
    assert!(matches!(
        second_ending,
        crate::ConversationUpdate::TurnEnded(_)
    ));

    let (first_answer, second_answer) = served.await.expect("the fixture completes");
    assert_eq!(first_answer["result"]["outcome"]["optionId"], "allow-once");
    assert_eq!(
        second_answer["result"]["outcome"]["optionId"],
        "reject-once"
    );
}

#[tokio::test]
async fn an_overlapping_prompt_keeps_ownership_of_its_permission_waiter() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a local port");
    let url = format!("ws://{}/", listener.local_addr().expect("an address"));
    let first_ended = Arc::new(tokio::sync::Notify::new());
    let first_ended_at_runtime = Arc::clone(&first_ended);
    let fixture_may_finish = Arc::new(tokio::sync::Notify::new());
    let finish_fixture = Arc::clone(&fixture_may_finish);

    let served = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.expect("a client");
        let mut ws = tokio_tungstenite::accept_async(socket)
            .await
            .expect("a websocket");
        ws.send(Message::Text(declaration(None)))
            .await
            .expect("the declaration is sent");

        let mut prompts = Vec::with_capacity(2);
        while prompts.len() < 2 {
            let call = receive_rpc(&mut ws).await;
            let method = call["method"].as_str().unwrap_or_default();
            if method == "session/prompt" {
                prompts.push(call["id"].clone());
                continue;
            }
            let result = if method == "session/new" {
                serde_json::json!({ "sessionId": "conv-overlapping-prompts" })
            } else {
                serde_json::json!({})
            };
            send_rpc(
                &mut ws,
                serde_json::json!({ "jsonrpc": "2.0", "id": call["id"], "result": result }),
            )
            .await;
        }

        send_rpc(
            &mut ws,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": prompts[0],
                "result": { "stopReason": "end_turn" },
            }),
        )
        .await;
        // Wait until the first prompt task has released its ownership. A Boolean
        // would now say there were no local prompt waiters even though the second
        // prompt is still pending.
        first_ended_at_runtime.notified().await;

        let request = crate::RequestPermissionRequest::new(
            crate::ConversationId::new("conv-overlapping-prompts"),
            crate::ToolCallUpdate::new(
                crate::ToolCallId::new("second-prompt-tool"),
                crate::ToolCallUpdateFields::new().title("second prompt action".to_string()),
            ),
            vec![
                crate::PermissionOption::new(
                    crate::PermissionOptionId::new("allow-once"),
                    "Allow once".to_string(),
                    crate::PermissionOptionKind::AllowOnce,
                ),
                crate::PermissionOption::new(
                    crate::PermissionOptionId::new("reject-once"),
                    "Reject once".to_string(),
                    crate::PermissionOptionKind::RejectOnce,
                ),
            ],
        );
        send_rpc(
            &mut ws,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": "permission-on-second-prompt",
                "method": "session/request_permission",
                "params": request,
            }),
        )
        .await;
        let permission_answer = receive_rpc(&mut ws).await;
        send_rpc(
            &mut ws,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": prompts[1],
                "result": { "stopReason": "end_turn" },
            }),
        )
        .await;
        // This deliberately unmatched response exposes a leaked orphan marker:
        // it must be ignored rather than reported as a third completed turn.
        send_rpc(
            &mut ws,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 9_999_999,
                "result": { "stopReason": "end_turn" },
            }),
        )
        .await;
        finish_fixture.notified().await;
        permission_answer
    });

    let stream = connect(&url).await;
    let (channel, _control, mut updates) =
        open_bridge(stream, "/home/chrono".into(), None, None, None, false)
            .await
            .expect("the bridge opens");
    for text in ["first prompt", "steer while the first is running"] {
        channel
            .input
            .send(crate::ConversationInput::Prompt(vec![
                crate::ContentBlock::from(text),
            ]))
            .await
            .expect("the overlapping prompt is sent");
    }

    let first_ending = tokio::time::timeout(std::time::Duration::from_secs(1), updates.recv())
        .await
        .expect("the first prompt ends")
        .expect("the update stream stays open");
    assert!(matches!(
        first_ending,
        crate::ConversationUpdate::TurnEnded(_)
    ));
    first_ended.notify_one();

    let asked = tokio::time::timeout(std::time::Duration::from_secs(1), updates.recv())
        .await
        .expect("the second prompt asks for permission")
        .expect("the update stream stays open");
    let crate::ConversationUpdate::Asked(asked) = asked else {
        panic!("expected the second prompt's permission request, got {asked:?}");
    };
    channel
        .input
        .send(crate::ConversationInput::PermissionAnswer {
            id: asked.id,
            locator: asked.locator,
            decision: crate::PermissionDecision::Allow,
        })
        .await
        .expect("the live permission answer is sent");

    let second_ending = tokio::time::timeout(std::time::Duration::from_secs(1), updates.recv())
        .await
        .expect("the second prompt ends")
        .expect("the update stream stays open");
    assert!(matches!(
        second_ending,
        crate::ConversationUpdate::TurnEnded(_)
    ));
    match tokio::time::timeout(std::time::Duration::from_millis(50), updates.recv()).await {
        Err(_) => {}
        Ok(None) => panic!("the update stream closed before the fixture finished"),
        Ok(Some(update)) => panic!("an unmatched response became a false turn ending: {update:?}"),
    }
    fixture_may_finish.notify_one();

    let answer = served.await.expect("the fixture completes");
    assert_eq!(
        answer["result"]["outcome"]["optionId"], "allow-once",
        "the live request receives the owner's decision"
    );
}

#[test]
fn a_turn_reports_the_reason_the_kernel_gave() {
    // The answer arriving here is the whole JSON-RPC frame — `inbound` hands
    // the receiver what it read. This path deserialized the envelope itself
    // into a stop reason, which could only ever fail, so every turn was
    // reported as `end_turn`: a refusal, a context limit and a cancellation
    // all looked like an agent that had finished normally.
    let super::Outcome::Ended(ended) = crate::hosted::turn::turn_outcome(serde_json::json!({
        "jsonrpc": "2.0",
        "id": 4,
        "result": { "stopReason": "max_tokens" },
    })) else {
        panic!("a turn the agent finished is an ending, not a failure");
    };

    assert!(
        matches!(ended.stop_reason, crate::TurnStopReason::MaxTokens),
        "got {:?}",
        ended.stop_reason
    );
}

#[test]
fn a_refused_prompt_is_not_reported_as_a_finished_turn() {
    // The worst of the flattened cases. Telling an owner their agent answered
    // when the bridge refused outright leaves them reading an empty turn and
    // wondering what they did wrong.
    //
    // The agent's own words are carried through, because that message is the
    // only part a person can act on — it is where "could not reach the model"
    // and an upstream 401 actually arrive.
    let super::Outcome::Failed { message, .. } =
        crate::hosted::turn::turn_outcome(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 4,
            "error": { "code": -32000, "message": "connection refused talking to chrono-llm" },
        }))
    else {
        panic!("a refusal is a failure, not a finished turn");
    };

    assert_eq!(message, "connection refused talking to chrono-llm");
}

#[test]
fn a_refusal_naming_a_capability_carries_the_name() {
    let super::Outcome::Failed { capability, .. } =
        crate::hosted::turn::turn_outcome(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 4,
            "error": { "code": -32021, "message": "no", "data": { "capability": "steer" } },
        }))
    else {
        panic!("a refusal is a failure");
    };

    assert_eq!(capability.as_deref(), Some("steer"));
}

#[test]
fn an_answer_this_build_cannot_read_still_closes_the_turn() {
    // A turn left open sits `running` forever and is indistinguishable from an
    // agent taking its time. Ending it is right even when the reason is not
    // legible; what must not happen is silence.
    let super::Outcome::Ended(ended) = crate::hosted::turn::turn_outcome(serde_json::json!({
        "jsonrpc": "2.0",
        "id": 4,
        "result": { "stopReason": "something-added-later" },
    })) else {
        panic!("a newer protocol naming a new ending must not read as a failure");
    };

    assert!(matches!(ended.stop_reason, crate::TurnStopReason::EndTurn));

    // An answer that is neither a result nor an error is not an ending: the
    // agent said nothing usable, and reporting that as a finished turn is how
    // an empty turn came to mean two different things.
    let empty = crate::hosted::turn::turn_outcome(serde_json::json!({ "jsonrpc": "2.0", "id": 4 }));
    assert!(matches!(empty, super::Outcome::Failed { .. }));
}

#[tokio::test]
async fn a_pod_whose_agent_never_answers_fails_the_handshake_quickly() {
    // The thirty-minute bound was sized for a prompt, which no longer travels
    // this path. Against a pod whose agent never started it held the session
    // at Attaching for half an hour with nothing said — indistinguishable
    // from a slow start, and the request that asked for it hung too.
    assert!(
        super::CALL_DEADLINE <= std::time::Duration::from_secs(60),
        "a handshake is a question with a quick answer: {:?}",
        super::CALL_DEADLINE
    );
    // A turn is the opposite and keeps its room, but is still bounded: a
    // prompt had no deadline at all, so a turn nobody would answer stayed
    // `running` forever.
    assert!(
        super::TURN_DEADLINE >= std::time::Duration::from_secs(600),
        "a turn takes as long as the model takes: {:?}",
        super::TURN_DEADLINE
    );
    assert!(
        super::TURN_DEADLINE < std::time::Duration::from_secs(24 * 3600),
        "but it is still bounded"
    );
}

/// The device-code method a bridge offers a client that can show a URL.
fn device_code_offer() -> serde_json::Value {
    serde_json::json!([
        { "id": "browser", "name": "Browser" },
        { "id": "chat-gpt-device-code", "name": "ChatGPT (device code)" }
    ])
}

#[tokio::test]
async fn an_own_account_session_signs_in_before_opening_a_conversation() {
    // Found in production, not here: the session provisioned, reached the
    // bridge, and died on `session/new` with "Authentication required". An
    // unauthenticated kernel has no account to open a conversation against, so
    // the sign-in cannot wait for somebody's first message.
    //
    // Deleting the call this guards left 171 tests green, which is why it
    // exists.
    let (url, served) = bridging_offering(None, None, Some(3), device_code_offer()).await;
    let stream = connect(&url).await;
    open_bridge(stream, "/home/chrono".into(), None, None, None, true)
        .await
        .expect("an own-account session opens");
    // Bounded, because the failure this guards against is a call that never
    // arrives — and waiting forever for it is not a test result. Unbounded,
    // deleting the sign-in made this hang instead of fail, which tells whoever
    // broke it nothing and stalls CI rather than reporting.
    let done = tokio::time::timeout(std::time::Duration::from_secs(10), served)
        .await
        .expect("the sign-in call never arrived")
        .expect("the fixture finished");

    let at = |m: &str| done.asked.iter().position(|a| a == m);
    assert_eq!(at("initialize"), Some(0));
    assert_eq!(
        at("authenticate"),
        Some(1),
        "the sign-in comes before the conversation, got {:?}",
        done.asked
    );
    assert!(
        at("authenticate") < at("session/new"),
        "a conversation opened before signing in is refused by the kernel, got {:?}",
        done.asked
    );
}

#[tokio::test]
async fn a_sign_in_request_is_reported_while_the_kernel_is_still_waiting() {
    /*
     * Found in production, and invisible to every test above.
     *
     * The kernel asks its owner to sign in from inside the handshake and then
     * blocks there until they have. When the update sink was created by this
     * function and handed back at the end, the request could only be delivered
     * after the open returned — after the sign-in it was asking for. The
     * session sat in `provisioning` showing nothing, the log had no
     * `sign_in_required` in it, and no client could draw the card that exists
     * to answer it.
     *
     * So the assertion is specifically about *when*: the update is read before
     * the open is awaited, with the fixture still holding the handshake.
     */
    let (url, served) = bridging_that_waits_for_a_sign_in().await;
    let stream = connect(&url).await;

    let (updates_tx, mut updates_rx) = mpsc::channel(64);
    let opening = tokio::spawn(Bridge::open(
        stream,
        "/home/chrono".into(),
        None,
        None,
        None,
        true,
        updates_tx,
    ));

    let reported = tokio::time::timeout(std::time::Duration::from_secs(10), updates_rx.recv())
        .await
        .expect("the sign-in was never reported while the kernel waited for it")
        .expect("the sink is still open");

    match reported {
        crate::ConversationUpdate::SignInRequired { url, .. } => {
            assert!(
                url.starts_with("https://"),
                "an address somebody can open, got {url:?}"
            );
        }
        other => {
            panic!("the first thing an own-account session reports is its sign-in, got {other:?}")
        }
    }

    opening
        .await
        .expect("the open task ran")
        .expect("an own-account session opens");
    let _ = tokio::time::timeout(std::time::Duration::from_secs(10), served).await;
}

#[tokio::test]
async fn a_delegated_session_never_signs_in() {
    // The platform's credential is attached by the proxy; asking a kernel to
    // sign in to an account nobody named would stall the session on a prompt
    // its owner has no way to answer.
    let (url, served) = bridging_offering(None, None, Some(2), device_code_offer()).await;
    let stream = connect(&url).await;
    open_bridge(stream, "/home/chrono".into(), None, None, None, false)
        .await
        .expect("a delegated session opens");
    let done = tokio::time::timeout(std::time::Duration::from_secs(10), served)
        .await
        .expect("the handshake never finished")
        .expect("the fixture finished");

    assert!(
        !done.asked.iter().any(|a| a == "authenticate"),
        "a delegated session must not sign in, got {:?}",
        done.asked
    );
}

#[tokio::test]
async fn a_bridge_offering_no_reachable_sign_in_is_refused_by_name() {
    // §9: an absence is declared rather than discovered. A browser method is
    // useless here — this pod has none, and a kernel that opened one would
    // hang — so a session that cannot sign in must not appear to start.
    let only_browser = serde_json::json!([{ "id": "browser", "name": "Browser" }]);
    let (url, served) = bridging_offering(None, None, Some(1), only_browser).await;
    let stream = connect(&url).await;
    let err = open_bridge(stream, "/home/chrono".into(), None, None, None, true)
        .await
        .expect_err("a session that cannot sign in must not open");
    drop(served);

    assert!(
        matches!(err, AdapterError::Unsupported(_)),
        "the refusal is a missing ability, got {err:?}"
    );
    assert!(
        err.to_string().contains("own account"),
        "and it names what cannot be done, got {err}"
    );
}
