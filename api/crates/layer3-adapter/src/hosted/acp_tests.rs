//! The handshake, against a bridge that answers.
//!
//! A real socket rather than a fake one, because what is being checked is the
//! ordering of a conversation with something that answers asynchronously: the
//! declaration is read first, the initialize is answered on the read loop that
//! was started before it was sent, and the mode — when there is one — is
//! settled before the caller is handed a channel it may prompt.

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

use tokio::sync::mpsc;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use super::Bridge;
use crate::supervisor;
use crate::AdapterError;

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
    switches: Option<(super::super::SwitchWaiters, String)>,
    own_credential: bool,
) -> Result<
    (
        crate::ConversationChannel,
        mpsc::Sender<crate::TerminalInput>,
        mpsc::Receiver<crate::ConversationUpdate>,
    ),
    AdapterError,
> {
    let (updates_tx, updates_rx) = mpsc::channel(64);
    let (channel, control) = Bridge::open(
        stream,
        cwd,
        mode,
        rejoin,
        switches,
        own_credential,
        updates_tx,
    )
    .await?;
    Ok((channel, control, updates_rx))
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
async fn every_request_this_bridge_sends_is_dispatchable() {
    // The general form, so the next request added cannot repeat it. A JSON-RPC
    // request without a method is undispatchable whatever it was meant to ask,
    // and the failure is silent at both ends.
    let (url, served) = bridging(None, None, Some(3)).await;
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
