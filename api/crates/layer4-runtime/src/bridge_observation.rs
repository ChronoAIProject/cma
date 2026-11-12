use std::sync::Arc;

use layer3_adapter::agent_capacity_recovery::ConfirmedAgentState;
use tokio::io::{AsyncReadExt as _, BufReader};
use tokio::sync::Mutex;

pub(super) async fn observe_input(
    conversation_id: &Arc<Mutex<Option<String>>>,
    native_state: &Arc<Mutex<Option<ConfirmedAgentState>>>,
    frame: &[u8],
) {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(frame) else {
        return;
    };
    let Some(method) = value.get("method").and_then(serde_json::Value::as_str) else {
        return;
    };
    if matches!(method, "session/load" | "session/new") {
        if let Some(session) = value
            .pointer("/params/sessionId")
            .and_then(serde_json::Value::as_str)
        {
            *conversation_id.lock().await = Some(session.to_owned());
        }
    }
    if method == "session/prompt" {
        let state = value
            .get("id")
            .and_then(serde_json::Value::as_str)
            .and_then(layer3_adapter::PromptIdentity::parse)
            .map(|prompt_id| ConfirmedAgentState::Working {
                prompt_id: prompt_id.as_str().to_owned(),
            });
        *native_state.lock().await = state;
    }
}

pub(super) async fn observe_output(
    conversation_id: &Arc<Mutex<Option<String>>>,
    native_state: &Arc<Mutex<Option<ConfirmedAgentState>>>,
    frame: &[u8],
) {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(frame) else {
        return;
    };
    if let Some(session) = value
        .pointer("/result/sessionId")
        .and_then(serde_json::Value::as_str)
    {
        *conversation_id.lock().await = Some(session.to_owned());
    }
    if value.get("method").and_then(serde_json::Value::as_str) == Some("session/request_permission")
    {
        let approval = value
            .pointer("/params/approvalId")
            .or_else(|| value.pointer("/params/approval_id"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        let prompt = match native_state.lock().await.clone() {
            Some(ConfirmedAgentState::Working { prompt_id }) => Some(prompt_id),
            _ => None,
        };
        *native_state.lock().await = prompt.zip(approval).map(|(prompt_id, approval_id)| {
            ConfirmedAgentState::WaitingApproval {
                prompt_id,
                approval_id,
            }
        });
    } else if value
        .get("id")
        .and_then(serde_json::Value::as_str)
        .is_some()
    {
        let mut state = native_state.lock().await;
        if matches!(&*state, Some(ConfirmedAgentState::Working { .. })) {
            *state = Some(ConfirmedAgentState::Idle);
        }
    }
}

pub(super) fn spawn_diagnostics(stderr: tokio::process::ChildStderr) {
    tokio::spawn(async move {
        let mut stderr = BufReader::new(stderr);
        let mut buffer = [0_u8; 8 * 1024];
        while let Ok(read) = stderr.read(&mut buffer).await {
            if read == 0 {
                break;
            }
            tracing::debug!(bytes = read, "agent bridge wrote diagnostics");
        }
    });
}

pub(super) fn spawn_reaper(
    mut child: tokio::process::Child,
    managed: crate::writer_fence::ManagedChild,
    ended: tokio::sync::watch::Sender<bool>,
) {
    tokio::spawn(async move {
        match child.wait().await {
            Ok(status) => tracing::info!(%status, "agent bridge exited"),
            Err(error) => tracing::warn!(%error, "agent bridge wait failed"),
        }
        drop(managed);
        ended.send_replace(true);
    });
}
