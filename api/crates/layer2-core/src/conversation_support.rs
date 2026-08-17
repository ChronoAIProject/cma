//! A runtime that holds a conversation, for tests that need to be the kernel.
//!
//! Split from `test_support` so both stay within the project's file-size
//! limit, and re-exported from there so a test still has one place to import
//! from. What it provides is the other end of the contract: a test pushes
//! updates and permission requests down, and reads back what the core sent up.

use layer3_adapter::{
    ContentBlock, ContentChunk, ConversationChannel, ConversationId, ConversationInput,
    ConversationUpdate, KernelCapabilities, PermissionOption, PermissionOptionId,
    PermissionOptionKind, PermissionRequest, RequestPermissionRequest, SessionUpdate, ToolCallId,
    ToolCallUpdate, ToolCallUpdateFields,
};
use std::sync::Arc;

use tokio::sync::{mpsc, Mutex};

use crate::authz::Caller;
use crate::event::EventKind;
use crate::ids::SessionId;
use crate::service::SessionService;
use crate::test_support::{await_settled, spec, user, FakeAdapter, FakeStore};

/// How many messages either direction may queue in a test.
const BUFFER: usize = 64;

/// The kernel's end of a conversation the core has opened.
pub(crate) struct FakeConversation {
    /// Updates to push toward the core.
    pub(crate) updates: mpsc::Sender<ConversationUpdate>,

    /// Everything the core has sent down, in order.
    pub(crate) received: std::sync::Arc<Mutex<Vec<ConversationInput>>>,
}

impl FakeConversation {
    /// Open a conversation, returning the core's end and the kernel's.
    pub(crate) fn open() -> (ConversationChannel, Self) {
        let (updates, update_rx) = mpsc::channel(BUFFER);

        let (input, mut input_rx) = mpsc::channel(BUFFER);
        let received = std::sync::Arc::new(Mutex::new(Vec::new()));

        let recorder = std::sync::Arc::clone(&received);
        tokio::spawn(async move {
            while let Some(sent) = input_rx.recv().await {
                recorder.lock().await.push(sent);
            }
        });

        (
            ConversationChannel {
                updates: update_rx,
                input,
            },
            Self { updates, received },
        )
    }

    /// Everything the core has sent down so far.
    pub(crate) async fn sent(&self) -> Vec<ConversationInput> {
        self.received.lock().await.clone()
    }
}

/// Something a runtime said, wrapped for the stream that carries it.
pub(crate) fn said(update: SessionUpdate) -> ConversationUpdate {
    ConversationUpdate::Said(update)
}

/// A message chunk, as a runtime streams one.
pub(crate) fn chunk(text: &str) -> ConversationUpdate {
    ConversationUpdate::Said(SessionUpdate::AgentMessageChunk(ContentChunk::new(
        ContentBlock::from(text),
    )))
}

/// A permission request a runtime is blocked on.
pub(crate) fn asking(id: &str, title: &str) -> PermissionRequest {
    PermissionRequest {
        id: id.to_string(),
        request: RequestPermissionRequest::new(
            ConversationId::new("sesn-under-test"),
            ToolCallUpdate::new(
                ToolCallId::new("call-1"),
                ToolCallUpdateFields::new().title(title.to_string()),
            ),
            vec![PermissionOption::new(
                PermissionOptionId::new("allow-once"),
                "Allow once".to_string(),
                PermissionOptionKind::AllowOnce,
            )],
        ),
    }
}

/// A kernel that holds a conversation and renders no terminal.
pub(crate) fn conversing() -> KernelCapabilities {
    KernelCapabilities {
        conversation: true,
        terminal: false,
        snapshot: true,
        steer: false,
        interrupt: true,
        usage: true,
    }
}

/// A live conversation session, and the runtime holding the other end.
pub(crate) async fn conversation() -> (Arc<SessionService>, Caller, SessionId, Arc<FakeAdapter>) {
    let adapter = Arc::new(FakeAdapter {
        capabilities: conversing(),
        ..FakeAdapter::default()
    });
    let profiles =
        crate::test_support::environments_defaulting_to(crate::environment::AgentConfiguration {
            kernel: crate::test_support::kernel(),
            mode: crate::options::SessionMode::Conversation,
            ..Default::default()
        });
    let svc = crate::test_support::service_with_environments(
        Arc::new(FakeStore::default()),
        Arc::clone(&adapter),
        profiles,
    );
    let alice = user("alice");
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;
    (svc, alice, created.id, adapter)
}

/// Wait until the runtime has been sent something matching `wanted`.
///
/// The channel is drained by a task, so reading it the instant after sending
/// would be reading before the runtime had a chance to receive anything.
pub(crate) async fn until_sent(
    adapter: &Arc<FakeAdapter>,
    wanted: impl Fn(&ConversationInput) -> bool,
) -> Vec<ConversationInput> {
    for _ in 0..400 {
        let sent = {
            let held = adapter.conversation.lock().await;
            match held.as_ref() {
                Some(kernel) => kernel.sent().await,
                None => Vec::new(),
            }
        };
        if sent.iter().any(&wanted) {
            return sent;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    Vec::new()
}

/// Wait until the log holds an event matching `wanted`.
pub(crate) async fn until_logged(
    svc: &Arc<SessionService>,
    caller: &Caller,
    id: &SessionId,
    wanted: impl Fn(&EventKind) -> bool,
) -> bool {
    for _ in 0..400 {
        if let Ok(events) = svc.events_since(caller, id, None).await {
            if events.iter().any(|e| wanted(&e.kind)) {
                return true;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    false
}
