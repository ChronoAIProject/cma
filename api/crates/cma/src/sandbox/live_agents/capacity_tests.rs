use super::super::*;
use layer2_core::{
    sandbox::{AgentConversationError, AgentMessageAssembly, AgentUpdateOutcome},
    RuntimeCapability,
};

struct Recorder;
#[async_trait::async_trait]
impl AgentUpdateRecorder for Recorder {
    async fn record_update(
        &self,
        _: &AgentId,
        _: u64,
        _: &mut AgentMessageAssembly,
        _: ConversationUpdate,
        _: String,
    ) -> Result<AgentUpdateOutcome, AgentConversationError> {
        Ok(AgentUpdateOutcome {
            appended: vec![],
            turn_closed: false,
        })
    }
}

fn staged(
    registry: &Arc<LiveAgentRegistry>,
    target: &LiveAgentTarget,
    fence: u64,
) -> (LiveAgentAttachment, mpsc::Sender<ConversationUpdate>) {
    let input = ConversationInputSender::spawn(4, |_| async { Ok(()) });
    let (sender, updates) = mpsc::channel(4);
    let attachment = registry
        .attach(
            target.agent_id.clone(),
            target.conversation_id.clone(),
            target.runtime_generation,
            fence,
            RuntimeCapabilities::new([RuntimeCapability::Conversation]),
            input,
            updates,
            Arc::new(Recorder),
        )
        .unwrap();
    (attachment, sender)
}
fn target() -> LiveAgentTarget {
    LiveAgentTarget {
        agent_id: AgentId::generate(),
        conversation_id: "retained-conversation".into(),
        runtime_generation: 3,
    }
}

#[tokio::test]
async fn only_exact_live_published_attachment_is_observed_without_promoting_staged_state() {
    let registry = Arc::new(LiveAgentRegistry::default());
    let target = target();
    assert!(!registry.recovery_attachment_is_active(&target, 7));
    let (first, _updates) = staged(&registry, &target, 7);
    assert!(!registry.recovery_attachment_is_active(&target, 7));
    registry.publish_and_activate(&first).unwrap();
    assert!(registry.recovery_attachment_is_active(&target, 7));
    for fence in [0, 6, 8] {
        assert!(!registry.recovery_attachment_is_active(&target, fence));
        assert!(registry.recovery_attachment_is_active(&target, 7));
    }
    for different in [
        LiveAgentTarget {
            agent_id: AgentId::generate(),
            ..target.clone()
        },
        LiveAgentTarget {
            conversation_id: "other-conversation".into(),
            ..target.clone()
        },
        LiveAgentTarget {
            runtime_generation: 0,
            ..target.clone()
        },
        LiveAgentTarget {
            runtime_generation: 2,
            ..target.clone()
        },
        LiveAgentTarget {
            runtime_generation: 4,
            ..target.clone()
        },
    ] {
        assert!(!registry.recovery_attachment_is_active(&different, 7));
    }
    let (next, _next_updates) = staged(&registry, &target, 8);
    // A staged successor advances the watermark but this observation cannot
    // publish it or reinterpret the previous connection under its new fence.
    for fence in [7, 8] {
        assert!(!registry.recovery_attachment_is_active(&target, fence));
    }
    registry.publish_and_activate(&next).unwrap();
    assert!(registry.recovery_attachment_is_active(&target, 8));
    registry.fence(&target.agent_id, Some(3), 8);
    assert!(!registry.recovery_attachment_is_active(&target, 8));
}

#[tokio::test]
async fn a_fenced_watermark_refuses_even_if_the_entry_is_still_present() {
    let registry = Arc::new(LiveAgentRegistry::default());
    let target = target();
    let (attached, _updates) = staged(&registry, &target, 7);
    registry.publish_and_activate(&attached).unwrap();
    {
        let mut state = registry.lock();
        state.watermarks.get_mut(&target.agent_id).unwrap().fenced = true;
    }
    assert!(!registry.recovery_attachment_is_active(&target, 7));
}

#[tokio::test]
async fn ended_task_and_lost_attachment_cannot_report_already_active() {
    let registry = Arc::new(LiveAgentRegistry::default());
    let target = target();
    let (attached, sender) = staged(&registry, &target, 7);
    registry.publish_and_activate(&attached).unwrap();
    let task = registry
        .lock()
        .entries
        .get(&target.agent_id)
        .unwrap()
        .task
        .clone();
    task.abort();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !task.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(!registry.recovery_attachment_is_active(&target, 7));
    drop(sender);
    registry.lock().entries.remove(&target.agent_id);
    assert!(!registry.recovery_attachment_is_active(&target, 7));
    // A new process's empty registry cannot establish native confirmation.
    assert!(!LiveAgentRegistry::default().recovery_attachment_is_active(&target, 7));
}
