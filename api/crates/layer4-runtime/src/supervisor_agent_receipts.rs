use crate::failure::Result;
use crate::mutation_store::Ticket;
use layer3_adapter::wire::{
    AgentCloseReceipt, AgentHomeReceipt, AgentHomeRemoveReceipt, AgentOpenReceipt, FencedReceipt,
    ResponseFrame,
};
use layer3_adapter::{AgentHomePlan, AgentLaunch};

pub(super) async fn complete_agent_home(
    authority_fence: u64,
    ticket: Ticket,
    plan: &AgentHomePlan,
    path: std::path::PathBuf,
) -> Result<ResponseFrame> {
    let receipt = AgentHomeReceipt {
        operation_id: plan.operation_id.clone(),
        request_sha256: plan.request_sha256.clone(),
        agent_id: plan.agent_id.clone(),
        path: path.to_string_lossy().into_owned(),
    };
    ticket.complete(&receipt).await?;
    Ok(home_response(authority_fence, receipt))
}

pub(super) async fn complete_agent_open(
    authority_fence: u64,
    ticket: Ticket,
    launch: AgentLaunch,
) -> Result<ResponseFrame> {
    let receipt = AgentOpenReceipt {
        operation_id: launch.operation_id,
        request_sha256: launch.request_sha256,
        agent_id: launch.agent_id,
        cwd: launch.cwd,
        home: launch.home,
        conversation_id: None,
    };
    ticket.complete(&receipt).await?;
    Ok(open_response(authority_fence, receipt))
}

pub(super) fn home_response(authority_fence: u64, receipt: AgentHomeReceipt) -> ResponseFrame {
    ResponseFrame::MaterializeAgentHome(FencedReceipt::new(authority_fence, receipt))
}

pub(super) fn remove_home_response(
    authority_fence: u64,
    receipt: AgentHomeRemoveReceipt,
) -> ResponseFrame {
    ResponseFrame::RemoveAgentHome(FencedReceipt::new(authority_fence, receipt))
}

pub(super) fn open_response(authority_fence: u64, receipt: AgentOpenReceipt) -> ResponseFrame {
    ResponseFrame::OpenAgent(FencedReceipt::new(authority_fence, receipt))
}

pub(super) fn close_response(authority_fence: u64, receipt: AgentCloseReceipt) -> ResponseFrame {
    ResponseFrame::CloseAgent(FencedReceipt::new(authority_fence, receipt))
}
