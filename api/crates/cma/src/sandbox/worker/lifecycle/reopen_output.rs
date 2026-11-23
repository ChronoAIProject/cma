use std::future::Future;
use std::time::Duration;

use futures_util::FutureExt as _;
use layer2_core::sandbox::store::AgentReopenGuard;
use layer2_core::sandbox::{AgentConversationError, AgentMessageAssembly, AgentUpdateRecorder};
use layer2_core::{now_rfc3339, AgentId};
use layer3_adapter::{AdapterError, ConversationUpdate};
use tokio::sync::mpsc;

/// Drain before open returns; ACK ownership passes only after a fenced commit.
pub(super) async fn drain_while<T>(
    operation: impl Future<Output = layer3_adapter::Result<T>>,
    receiver: &mut mpsc::Receiver<ConversationUpdate>,
    recorder: &dyn AgentUpdateRecorder,
    agent_id: &AgentId,
    guard: &AgentReopenGuard,
) -> layer3_adapter::Result<T> {
    if layer2_core::sandbox::store::WorkAuthority::active() {
        layer2_core::sandbox::store::WorkAuthority::current()
            .map_err(|_| AdapterError::Unavailable("ordinary work authority expired".into()))?;
        // The enclosing worker independently renews and cancels this future.
        // A captured wall-clock expiry cannot shorten a renewed long operation.
        return tokio::time::timeout(
            Duration::from_secs(1_800),
            drain(operation, receiver, recorder, agent_id, guard),
        )
        .await
        .map_err(|_| AdapterError::Unavailable("reopen output operation timed out".into()))?;
    }
    let expires = guard
        .sandbox
        .lease
        .as_ref()
        .and_then(|lease| layer2_core::seconds_from_rfc3339(&lease.expires_at))
        .ok_or_else(|| {
            AdapterError::Rejected("reopen output requires a finite lifecycle lease".into())
        })?;
    let now = layer2_core::seconds_from_rfc3339(&now_rfc3339())
        .expect("clock produces a valid timestamp");
    let remaining = expires.saturating_sub(now);
    // Tokio polls an immediately ready future even when its timeout is zero.
    // Refuse before polling so an expired claim cannot begin the operation.
    if remaining == 0 {
        return Err(AdapterError::Unavailable(
            "reopen output lifecycle lease expired".into(),
        ));
    }
    tokio::time::timeout(
        Duration::from_secs(remaining),
        drain(operation, receiver, recorder, agent_id, guard),
    )
    .await
    .map_err(|_| AdapterError::Unavailable("reopen output lifecycle lease expired".into()))?
}

/// Kept below the caller's 60 s relay timeout, so the owner returns a typed
/// refusal instead of being cut off mid-operation.
pub(super) const RELAYED_REOPEN_LIMIT: Duration = Duration::from_secs(55);

/// A relayed owner has no caller `WorkAuthority`, and the caller's 15 s work
/// lease is renewed in place where the owner cannot see it (#3244). Bound the
/// reopen by that durable lease instead of its snapshot: continue while `held`
/// confirms the caller's exact lease is still held, refuse as soon as it is
/// not, and never exceed `RELAYED_REOPEN_LIMIT`. Every durable reopen write
/// remains fenced by `guard` at the write; this bound only stops the work.
pub(super) async fn drain_while_held<T, H, Held>(
    operation: impl Future<Output = layer3_adapter::Result<T>>,
    receiver: &mut mpsc::Receiver<ConversationUpdate>,
    recorder: &dyn AgentUpdateRecorder,
    agent_id: &AgentId,
    guard: &AgentReopenGuard,
    mut held: H,
) -> layer3_adapter::Result<T>
where
    H: FnMut() -> Held,
    Held: Future<Output = bool>,
{
    let lost = || AdapterError::Unavailable("reopen output lifecycle lease expired".into());
    // Refuse before polling, so a lapsed lease cannot begin the operation.
    if !held().await {
        return Err(lost());
    }
    let watch = async {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if !held().await {
                return;
            }
        }
    };
    tokio::select! {
        biased;
        () = watch => Err(lost()),
        result = tokio::time::timeout(
            RELAYED_REOPEN_LIMIT,
            drain(operation, receiver, recorder, agent_id, guard),
        ) => result.map_err(|_| AdapterError::Unavailable("reopen output operation timed out".into()))?,
    }
}

async fn drain<T>(
    operation: impl Future<Output = layer3_adapter::Result<T>>,
    receiver: &mut mpsc::Receiver<ConversationUpdate>,
    recorder: &dyn AgentUpdateRecorder,
    agent_id: &AgentId,
    guard: &AgentReopenGuard,
) -> layer3_adapter::Result<T> {
    tokio::pin!(operation);
    let mut assembly = AgentMessageAssembly::default();
    loop {
        tokio::select! {
            biased;
            result = &mut operation => return result,
            update = receiver.recv() => {
                let Some(update) = update else {
                    return prefer_opening_failure(operation.as_mut(), AdapterError::Unavailable("reopen output receiver ended".into()));
                };
                let receipt = match &update {
                    ConversationUpdate::RuntimeOutput { receipt, .. } => Some(receipt),
                    ConversationUpdate::Lost { .. } => return Err(AdapterError::Unavailable("reopen output contains a gap".into())),
                    _ => None,
                };
                let recording = async {
                    loop {
                        match recorder.record_reopen_update(agent_id, guard, &mut assembly, update.clone(), now_rfc3339()).await {
                            Ok(_) => break,
                            Err(AgentConversationError::Unavailable(_)) => tokio::time::sleep(Duration::from_secs(1)).await,
                            Err(error) => return Err(AdapterError::Unavailable(format!("durable reopen output was refused: {error}"))),
                        }
                    }
                    if let Some(receipt) = receipt {
                        receipt.acknowledge().await?;
                    }
                    Ok(())
                };
                tokio::pin!(recording);
                let mut completed = None;
                let recorded = loop {
                    // Tool confirmation inside the opener can hold the Agent
                    // writer needed by this receipt and by accepted stop work.
                    // Drive it through recording/retry/ACK, but finish custody
                    // of this update before handing off a completed channel.
                    tokio::select! {
                        result = &mut operation, if completed.is_none() => completed = Some(result),
                        result = &mut recording => break result,
                    }
                };
                if let Err(error) = recorded {
                    return match completed {
                        Some(Err(original)) => Err(original),
                        Some(Ok(_)) => Err(error),
                        None => prefer_opening_failure(operation.as_mut(), error),
                    };
                }
                if let Some(result) = completed {
                    return result;
                }
            }
        }
    }
}

/// Opening may have failed while the fenced recorder was committing. Its
/// cleanup closes ACK ownership, but must not replace the original refusal.
/// Poll once: an unresolved open cannot delay custody failure, and even an
/// opening success cannot make a failed output acknowledgement successful.
fn prefer_opening_failure<T>(
    operation: impl Future<Output = layer3_adapter::Result<T>>,
    output_error: AdapterError,
) -> layer3_adapter::Result<T> {
    match operation.now_or_never() {
        Some(Err(error)) => Err(error),
        Some(Ok(_)) | None => Err(output_error),
    }
}

#[cfg(test)]
#[path = "reopen_output_tests.rs"]
mod tests;
