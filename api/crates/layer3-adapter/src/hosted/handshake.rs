//! The opening exchange with a runtime's bridge, and signing it in.
//!
//! Split from `acp.rs` so both stay within the project's file-size limit, on a
//! division that is real: that file owns the link — the socket, the read loop,
//! the pending calls — and this one owns the conversation that link is opened
//! to have. A wrong exchange here costs a session its history, which is worth
//! being able to read on its own.

use crate::AdapterError;

use super::acp::{Bridge, PROTOCOL_VERSION, SIGN_IN_DEADLINE};

impl Bridge {
    /// Initialize, get hold of a session, and settle its mode.
    ///
    /// Three ways to have one, and they are genuinely different. A bridge that
    /// is still running holds its session in memory and is simply told which
    /// one this is. A bridge that has just started, on a pod handed a
    /// conversation, loads it from disk. Anything else opens a new one.
    ///
    /// Getting the first two the wrong way round fails, and was checked rather
    /// than reasoned about: a live bridge refuses to load a session it is
    /// already holding, because loading reads a file that a conversation with
    /// no committed turns has not written yet.
    /// Open the exchange, and accept a bridge that is already open.
    ///
    /// `initialize` is once per bridge, not once per connection, and a bot pod
    /// outlives the control plane that started it. So a control plane taking
    /// back over a live runtime — every rollout, every restart — meets a
    /// kernel that was initialized by its predecessor and says so.
    ///
    /// That refusal is the answer we want. It is only reachable from a process
    /// that exists and is dispatching, which is exactly what the greeting is
    /// asking. Failing on it cost a live session its history on every deploy:
    /// the rejection was read as the sandbox being gone, and a fresh one was
    /// provisioned over the top of a runtime that was still working (#416).
    ///
    /// **Nothing is recognised by its message.** The discriminator is whether
    /// a reply frame came back at all: `call_within` produces `Rejected` only
    /// after a frame has arrived and been matched to this call, and silence —
    /// a deadline, a closed link — produces `Unavailable` instead. A kernel
    /// may reword its refusal, change its code, or refuse for a reason nobody
    /// has seen, and this still reads correctly. Matching on prose would start
    /// destroying sessions again the day the wording changed, during a
    /// rollout, with nobody watching. It would also have covered one kernel
    /// and not the other, since nothing here has ever observed what the second
    /// says when it is greeted twice.
    ///
    /// The reply is `null` in that case rather than a declaration. Every
    /// caller of it already handles a declaration that says nothing, because a
    /// bridge is allowed not to advertise.
    async fn greet(&self, params: serde_json::Value) -> Result<serde_json::Value, AdapterError> {
        greeted(self.call("initialize", params).await)
    }

    pub(super) async fn handshake(
        &self,
        rejoining: Option<String>,
        resuming: Option<String>,
        cwd: String,
        mode: Option<String>,
        own_credential: bool,
    ) -> Result<(), AdapterError> {
        let hello = self
            .greet(serde_json::json!({
            "protocolVersion": PROTOCOL_VERSION,
            "clientCapabilities": {
                "fs": { "readTextFile": false, "writeTextFile": false },
                // Declared so the bridge will offer a sign-in this client
                // can actually complete. It advertises its device-code
                // method only to a client that says it can put a URL in
                // front of a person, and this client can: the address and
                // the code travel up as an update and reach a surface. Say
                // nothing here and the method is never offered, which is
                // why an own-account session had no way to sign in.
                "elicitation": { "url": {} },
            },
            }))
            .await?;

        /*
         * A session on its owner's own account signs in before anything else.
         *
         * Found by running it: the conversation reached the bridge and died on
         * `session/new` with "Authentication required". An unauthenticated
         * kernel has no account to open a conversation against, so the sign-in
         * cannot wait until somebody sends a first message — there is nothing
         * to send it to.
         *
         * Skipped when the pod is already holding a conversation, which is a
         * runtime that signed in earlier and is being taken back over.
         */
        if own_credential && rejoining.is_none() {
            self.authenticate_owner(&hello).await?;
        }

        let session = match (rejoining, resuming) {
            // A conversation this bridge is still holding. Nothing is asked of
            // it beyond the identifier: the session is in the process's memory,
            // and asking it to load one would be asking it to read a file it
            // may not have written.
            (Some(conversation), _) => {
                tracing::info!(%conversation, "rejoining the conversation this runtime is holding");
                conversation
            }
            // Carried from another pod. The rollout is already on this pod's
            // disk — the supervisor put it there — so loading by identifier is
            // what makes the session continue rather than start again.
            (None, Some(conversation)) => {
                let params = serde_json::json!({
                    "sessionId": conversation,
                    "cwd": cwd,
                    "mcpServers": [],
                });
                self.call("session/load", params).await.map_err(|e| {
                    // Loud: the runtime this conversation came from is about to
                    // be torn down, and reporting success here would lose it.
                    //
                    // `context` rather than a new variant: a deadline and a
                    // closed link are both silence, and rewording them as a
                    // refusal told the core the runtime had answered when
                    // nothing had.
                    e.context("could not resume conversation")
                })?;
                conversation
            }
            (None, None) => {
                let started = self
                    .call(
                        "session/new",
                        serde_json::json!({ "cwd": cwd, "mcpServers": [] }),
                    )
                    .await?;
                started
                    .get("sessionId")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| {
                        AdapterError::Rejected("the bridge named no session".to_string())
                    })?
                    .to_string()
            }
        };
        *self.session.lock().await = Some(session.clone());

        // Refused rather than defaulted, for the bridges that have modes: they
        // start every session in their most permissive one whatever the
        // kernel's own configuration says, so a session whose mode did not
        // take would be acting unattended while its owner had asked to be
        // asked. A bridge with no modes names none and this is skipped.
        if let Some(mode) = mode {
            self.call(
                "session/set_mode",
                serde_json::json!({ "sessionId": session, "modeId": mode }),
            )
            .await
            .map_err(|e| {
                e.context(&format!(
                    "this session could not be held to its permission mode ({mode})"
                ))
            })?;
            tracing::info!(%session, %mode, "conversation opened");
        } else {
            tracing::info!(%session, "conversation opened");
        }
        Ok(())
    }

    /// Sign the agent in to its owner's own account, and wait for it.
    ///
    /// The method is chosen from what the bridge advertised rather than named
    /// here: which sign-ins a kernel offers is the kernel's business, and a
    /// client that hardcoded one would break the first time that changed. A
    /// bridge offering none is refused by name — §9 asks for an absence to be
    /// declared, and a session that cannot sign in cannot run on the account it
    /// was created for.
    ///
    /// This blocks until the sign-in finishes, which is the point: the address
    /// and the code have already reached a surface by then, carried up by the
    /// elicitation the bridge sends while it waits.
    async fn authenticate_owner(&self, hello: &serde_json::Value) -> Result<(), AdapterError> {
        let methods = hello
            .get("authMethods")
            .and_then(serde_json::Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let chosen = methods
            .iter()
            .filter_map(|m| m.get("id").and_then(serde_json::Value::as_str))
            // A device code is the only kind completable from here: this pod
            // has no browser, and a method that opened one would hang.
            .find(|id| id.contains("device"))
            .ok_or_else(|| {
                AdapterError::Unsupported(
                    "this runtime offers no sign-in that can be completed away from the machine \
                     it runs on, so it cannot use its owner's own account"
                        .to_string(),
                )
            })?
            .to_string();

        tracing::info!(method = %chosen, "signing in to the owner's own account");
        self.call_within(
            "authenticate",
            serde_json::json!({ "methodId": chosen }),
            SIGN_IN_DEADLINE,
        )
        .await
        .map_err(|e| {
            // Named rather than swallowed. A refused or expired sign-in is
            // the likeliest way this fails, and the person who has to act
            // on it is the one reading the session.
            e.context("the sign-in did not complete")
        })?;
        Ok(())
    }
}

/// Read a greeting's outcome: an answer, a refusal, or silence.
///
/// Free rather than a method, and this is deliberate — it is the one decision
/// in the handshake that must be exercised in both directions, and taking a
/// `Result` lets a test hand it silence without a socket that has to time out
/// to produce some. A socket-level test of this cannot tell the two apart:
/// under either behaviour the link is dead and the *next* call fails as loss
/// anyway, so the assertion passes for the wrong reason.
fn greeted(
    outcome: Result<serde_json::Value, AdapterError>,
) -> Result<serde_json::Value, AdapterError> {
    match outcome {
        Ok(hello) => Ok(hello),
        Err(refused @ AdapterError::Rejected(_)) => {
            tracing::info!(
                refusal = %refused,
                "this runtime was already greeted; taking over the bridge it is holding"
            );
            Ok(serde_json::Value::Null)
        }
        // Silence, or a stated absence. Neither is proof of a live bridge, and
        // reading them as one leaves a session pointed at a sandbox that is
        // gone and never replaced.
        Err(quiet) => Err(quiet),
    }
}

#[cfg(test)]
#[path = "handshake_tests.rs"]
mod handshake_tests;
