//! The handshake, against a bridge that answers.
//!
//! A real socket rather than a fake one, because what is being checked is the
//! ordering of a conversation with something that answers asynchronously: the
//! declaration is read first, the initialize is answered on the read loop that
//! was started before it was sent, and the mode — when there is one — is
//! settled before the caller is handed a channel it may prompt.

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

use super::Bridge;
use crate::supervisor;

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

#[tokio::test]
async fn a_fresh_conversation_is_started_and_held_to_its_mode() {
    let (url, served) = bridging(None, None, Some(3)).await;
    let stream = connect(&url).await;
    Bridge::open(
        stream,
        "/home/chrono".into(),
        Some("read-only".into()),
        None,
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
    Bridge::open(stream, "/home/chrono".into(), Some("agent".into()), None)
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
    Bridge::open(stream, "/home/chrono".into(), None, None)
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
    let err = Bridge::open(
        stream,
        "/home/chrono".into(),
        Some("read-only".into()),
        None,
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
    let err = Bridge::open(stream, "/home/chrono".into(), Some("agent".into()), None)
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
    let err = Bridge::open(stream, "/home/chrono".into(), None, None)
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
    Bridge::open(
        stream,
        "/home/chrono".into(),
        Some("read-only".into()),
        Some("conv-already-running".into()),
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
    Bridge::open(
        stream,
        "/home/chrono".into(),
        Some("agent".into()),
        Some("conv-already-running".into()),
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
    let (channel, _control) = Bridge::open(stream, "/home/chrono".into(), None, None)
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
    let (channel, _control) =
        Bridge::open(stream, "/home/chrono".into(), Some("agent".into()), None)
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
    let ended = super::turn_outcome(serde_json::json!({
        "jsonrpc": "2.0",
        "id": 4,
        "result": { "stopReason": "max_tokens" },
    }));

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
    let ended = super::turn_outcome(serde_json::json!({
        "jsonrpc": "2.0",
        "id": 4,
        "error": { "code": -32000, "message": "no" },
    }));

    assert!(
        matches!(ended.stop_reason, crate::TurnStopReason::Refusal),
        "got {:?}",
        ended.stop_reason
    );
}

#[test]
fn an_answer_this_build_cannot_read_still_closes_the_turn() {
    // A turn left open sits `running` forever and is indistinguishable from an
    // agent taking its time. Ending it is right even when the reason is not
    // legible; what must not happen is silence.
    let ended = super::turn_outcome(serde_json::json!({
        "jsonrpc": "2.0",
        "id": 4,
        "result": { "stopReason": "something-added-later" },
    }));

    assert!(matches!(ended.stop_reason, crate::TurnStopReason::EndTurn));

    let empty = super::turn_outcome(serde_json::json!({ "jsonrpc": "2.0", "id": 4 }));
    assert!(matches!(empty.stop_reason, crate::TurnStopReason::EndTurn));
}
