use super::*;
use crate::store::agent_access_fixtures as fixtures;
use layer2_core::sandbox::{AgentStatus, AgentStopReason, FirstMessageRecord};

fn open_pair() -> (Agent, Agent) {
    let sandbox = fixtures::sandbox("prepared-claim");
    let workspace = fixtures::workspace(&sandbox, "prepared-claim");
    let mut completed = fixtures::admission(&workspace, "prepared-claim")
        .materialize(&workspace)
        .unwrap()
        .agent;
    completed
        .work
        .schedule(AgentWorkStep::OpenConversation, fixtures::NOW.into());
    let mut claimed = completed.clone();
    claimed
        .work
        .claim(
            "worker".into(),
            fixtures::NOW,
            "2026-09-05T00:01:00Z".into(),
        )
        .unwrap();
    claimed.revision += 1;
    (completed, claimed)
}

#[test]
fn prepared_claim_accepts_only_the_next_claim_and_owner_startup_metadata() {
    let (completed, mut claimed) = open_pair();
    assert!(same_prepared_open(&completed, &claimed));
    claimed.title = Some("Owner title".into());
    claimed.conversation.first_message =
        Some(FirstMessageRecord::new(&claimed.creation_key, "Run once", fixtures::NOW).unwrap());
    claimed.conversation.last_seq += 1;
    claimed.revision += 1;
    assert!(same_prepared_open(&completed, &claimed));
    for variant in 0..9 {
        let mut changed = claimed.clone();
        match variant {
            0 => changed.work.lease = None,
            1 => {
                changed.work.lease.as_mut().unwrap().fence += 1;
                changed.work.next_fence += 1;
            }
            2 => changed.work.step = AgentWorkStep::FenceInteraction,
            3 => changed.requested_stop_reason = Some(AgentStopReason::Owner),
            4 => changed.status = AgentStatus::Stopped,
            5 => changed.profile.content.system_prompt = "Changed settings".into(),
            6 => changed.work.warm_conversation_id = Some("different-conversation".into()),
            7 => changed.sandbox_id = layer2_core::SandboxId::generate(),
            8 => changed.conversation.conversation_id = Some("already-open".into()),
            _ => unreachable!(),
        }
        assert!(
            !same_prepared_open(&completed, &changed),
            "variant {variant}"
        );
    }
}
