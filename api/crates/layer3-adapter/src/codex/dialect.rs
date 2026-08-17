//! Which wire a runtime speaks, and how terminal input is rendered onto it.
//!
//! Split from the adapter so that file stays within the project's size limit.
//! The division is real: that file drives a runtime's lifecycle, and this
//! settles how one is talked to once it exists.
//!
//! Two dialects, because CLAUDE.md §5 has both halves of a rollout in flight
//! at once — pods that predate lanes running against a control plane that
//! knows them, and the reverse. Neither guesses: the control plane asks for the
//! wire it speaks, and a pod that knows lanes answers by declaring itself.

use futures_util::StreamExt;
use tokio_tungstenite::tungstenite::Message;

use crate::supervisor;
use crate::TerminalInput;

use super::{CodexAdapter, DIALECT_DEADLINE};

impl CodexAdapter {
    /// Learn which wire this pod speaks, from the first thing it says.
    ///
    /// A pod that knows lanes declares itself before anything else, so a
    /// declaration arrives immediately or not at all. A pod that does not
    /// simply starts relaying its terminal, which is itself the answer — and
    /// the bytes it sent while answering are handed back rather than dropped,
    /// because they are the first thing on somebody's screen.
    ///
    /// Waiting is bounded, and the bound is only ever paid by an older pod
    /// whose agent happens to be silent at startup. A newer one answers at
    /// once; an older one usually answers with output just as fast.
    pub(super) async fn negotiate<S>(source: &mut S) -> (bool, Option<Vec<u8>>)
    where
        S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
    {
        match tokio::time::timeout(DIALECT_DEADLINE, source.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => {
                match supervisor::Ready::from_control(&text) {
                    Some(ready) => {
                        tracing::info!(
                            wire = ready.wire_version,
                            lanes = ?ready.lanes,
                            "the runtime declared its wire"
                        );
                        (true, None)
                    }
                    // Some other control frame. Nothing is inferred from it:
                    // the older wire is the reading a pod cannot misunderstand.
                    None => (false, None),
                }
            }
            // Terminal output before any declaration means there was never
            // going to be one.
            Ok(Some(Ok(Message::Binary(bytes)))) => (false, Some(bytes.to_vec())),
            Ok(Some(Ok(_))) => (false, None),
            Ok(Some(Err(_))) | Ok(None) => (false, None),
            Err(_) => {
                tracing::info!("the runtime declared no wire; using the one that predates lanes");
                (false, None)
            }
        }
    }

    /// Render one piece of terminal input for the wire this link speaks.
    ///
    /// Only the data lane is framed. Control travels as text and is identical
    /// on both dialects, which is what lets a credential reach a pod that
    /// predates lanes and one that does not without either being told which it
    /// is talking to.
    pub(super) fn encode(input: TerminalInput, framed: bool) -> Message {
        match input {
            TerminalInput::Bytes(b) => Message::Binary(match framed {
                true => supervisor::frame(supervisor::LANE_TERMINAL, &b),
                false => b,
            }),
            TerminalInput::Resize(size) => Message::Text(format!(
                r#"{{"t":"resize","cols":{},"rows":{}}}"#,
                size.cols, size.rows
            )),
            TerminalInput::Signal(sig) => {
                Message::Text(format!(r#"{{"t":"signal","sig":"{sig}"}}"#))
            }
            TerminalInput::Credential(value) => {
                Message::Text(serde_json::json!({ "t": "credential", "value": value }).to_string())
            }
        }
    }
}
