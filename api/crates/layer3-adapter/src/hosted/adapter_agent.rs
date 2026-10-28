use std::path::Path;

use tokio::sync::mpsc;

use super::super::reply::{valid_git_commit, valid_sha256};
use super::HostedAdapter;
use crate::wire::{FencedRequest, RequestFrame, ResponseFrame};
use crate::{
    AdapterError, AgentCloseRequest, AgentHomeRemoveRequest, AgentHomeRequest, AgentOpenRequest,
    ConversationChannel, ConversationUpdate, RuntimeHandle, WorktreeReceipt, WorktreeRef,
    WorktreeRequest,
};

pub(super) async fn add_worktree(
    adapter: &HostedAdapter,
    handle: &RuntimeHandle,
    authority_fence: u64,
    request: &WorktreeRequest,
) -> crate::Result<WorktreeReceipt> {
    validate_fence(authority_fence)?;
    let state = adapter.known_state(handle).await?;
    validate_request(
        request,
        format!("worktree:{}:{}:add:v1", request.workspace, request.name),
    )?;
    match adapter
        .exchange(
            handle,
            RequestFrame::AddWorktree(FencedRequest::new(authority_fence, request.clone())),
        )
        .await?
    {
        ResponseFrame::AddWorktree(response)
            if response.authority_fence == authority_fence
                && response.receipt.operation_id == request.operation_id
                && response.receipt.request_sha256 == request.request_sha256
                && response.receipt.workspace == request.workspace
                && response.receipt.name == request.name
                && Path::new(&response.receipt.path)
                    == Path::new(&state.home_directory)
                        .join(&request.workspace)
                        .join(".worktrees")
                        .join(&request.name)
                && response.receipt.branch == format!("cma/{}", request.name)
                && valid_git_commit(&response.receipt.base_commit) =>
        {
            Ok(response.receipt)
        }
        ResponseFrame::AddWorktree(_) => Err(AdapterError::Rejected(
            "runtime returned a receipt for another worktree addition".into(),
        )),
        _ => unreachable!("exchange verifies response operation"),
    }
}

pub(super) async fn remove_worktree(
    adapter: &HostedAdapter,
    handle: &RuntimeHandle,
    authority_fence: u64,
    request: &WorktreeRef,
) -> crate::Result<()> {
    validate_fence(authority_fence)?;
    adapter.known_state(handle).await?;
    validate_request(
        request,
        format!("worktree:{}:{}:remove:v1", request.workspace, request.name),
    )?;
    match adapter
        .exchange(
            handle,
            RequestFrame::RemoveWorktree(FencedRequest::new(authority_fence, request.clone())),
        )
        .await?
    {
        ResponseFrame::RemoveWorktree(response)
            if response.authority_fence == authority_fence
                && response.receipt.operation_id == request.operation_id
                && response.receipt.request_sha256 == request.request_sha256
                && response.receipt.workspace == request.workspace
                && response.receipt.name == request.name =>
        {
            Ok(())
        }
        ResponseFrame::RemoveWorktree(_) => Err(AdapterError::Rejected(
            "runtime returned a receipt for another worktree removal".into(),
        )),
        _ => unreachable!("exchange verifies response operation"),
    }
}

pub(super) async fn materialize_home(
    adapter: &HostedAdapter,
    handle: &RuntimeHandle,
    authority_fence: u64,
    request: &AgentHomeRequest,
) -> crate::Result<()> {
    validate_fence(authority_fence)?;
    adapter.validate_options(handle, &request.options).await?;
    validate_request(
        request,
        format!("agent:{}:home:create:v1", request.options.slot.as_str()),
    )?;
    let plan = adapter.runtime.home_plan(request)?;
    if plan.operation_id != request.operation_id
        || plan.request_sha256 != request.request_sha256
        || plan.agent_id != request.options.slot
    {
        return Err(AdapterError::Rejected(
            "agent home plan changed the requested operation or slot".into(),
        ));
    }
    match adapter
        .exchange(
            handle,
            RequestFrame::MaterializeAgentHome(FencedRequest::new(authority_fence, plan)),
        )
        .await?
    {
        ResponseFrame::MaterializeAgentHome(response)
            if response.authority_fence == authority_fence
                && response.receipt.operation_id == request.operation_id
                && response.receipt.request_sha256 == request.request_sha256
                && response.receipt.agent_id == request.options.slot
                && Path::new(&response.receipt.path)
                    == Path::new(&request.options.home_directory) =>
        {
            Ok(())
        }
        ResponseFrame::MaterializeAgentHome(_) => Err(AdapterError::Rejected(
            "runtime returned a receipt for another agent home".into(),
        )),
        _ => unreachable!("exchange verifies response operation"),
    }
}

pub(super) async fn install_skill(
    adapter: &HostedAdapter,
    handle: &RuntimeHandle,
    authority_fence: u64,
    request: &crate::AgentSkillInstallRequest,
) -> crate::Result<()> {
    validate_fence(authority_fence)?;
    adapter.known_state(handle).await?;
    if !request.has_valid_sha256()
        || request.operation_id
            != format!(
                "agent:{}:skill:{}:install:v1",
                request.agent_id.as_str(),
                request.skill.id
            )
    {
        return Err(AdapterError::Rejected(
            "invalid skill installation identity".into(),
        ));
    }
    match adapter
        .exchange(
            handle,
            RequestFrame::InstallAgentSkill(FencedRequest::new(authority_fence, request.clone())),
        )
        .await?
    {
        ResponseFrame::InstallAgentSkill(response)
            if response.authority_fence == authority_fence && response.receipt.matches(request) =>
        {
            Ok(())
        }
        _ => Err(AdapterError::Rejected(
            "runtime returned an uncorrelated skill receipt".into(),
        )),
    }
}

pub(super) async fn remove_home(
    adapter: &HostedAdapter,
    handle: &RuntimeHandle,
    authority_fence: u64,
    request: &AgentHomeRemoveRequest,
) -> crate::Result<()> {
    validate_fence(authority_fence)?;
    adapter.known_state(handle).await?;
    validate_request(
        request,
        format!("agent:{}:home:remove:v1", request.agent_id.as_str()),
    )?;
    match adapter
        .exchange(
            handle,
            RequestFrame::RemoveAgentHome(FencedRequest::new(authority_fence, request.clone())),
        )
        .await?
    {
        ResponseFrame::RemoveAgentHome(response)
            if response.authority_fence == authority_fence
                && response.receipt.operation_id == request.operation_id
                && response.receipt.request_sha256 == request.request_sha256
                && response.receipt.agent_id == request.agent_id =>
        {
            Ok(())
        }
        ResponseFrame::RemoveAgentHome(_) => Err(AdapterError::Rejected(
            "runtime returned a receipt for another agent home removal".into(),
        )),
        _ => unreachable!("exchange verifies response operation"),
    }
}

pub(super) async fn reconnect_agent(
    adapter: &HostedAdapter,
    handle: &RuntimeHandle,
    authority_fence: u64,
    request: &AgentOpenRequest,
    (conversation_id, recovered_prompt): (&str, Option<crate::PromptIdentity>),
    updates: mpsc::Sender<ConversationUpdate>,
) -> crate::Result<ConversationChannel> {
    let result = open_agent(
        adapter,
        handle,
        authority_fence,
        request,
        Some((conversation_id, recovered_prompt.clone())),
        updates.clone(),
    )
    .await;
    match result {
        Err(AdapterError::OperationIdentityConflict(_))
            if request.resume_conversation.is_none() =>
        {
            // Older restores included resume identity in the durable launch digest.
            // Only an exact pre-effect refusal permits this one canonical replay.
            let resumed = AgentOpenRequest::new(
                request.operation_id.clone(),
                request.options.clone(),
                Some(conversation_id.to_owned()),
            );
            open_agent(
                adapter,
                handle,
                authority_fence,
                &resumed,
                Some((conversation_id, recovered_prompt)),
                updates,
            )
            .await
        }
        result => result,
    }
}

pub(super) async fn open_agent(
    adapter: &HostedAdapter,
    handle: &RuntimeHandle,
    authority_fence: u64,
    request: &AgentOpenRequest,
    held: Option<(&str, Option<crate::PromptIdentity>)>,
    updates: mpsc::Sender<ConversationUpdate>,
) -> crate::Result<ConversationChannel> {
    let (held_conversation, recovered_prompt) = match held {
        Some((conversation, prompt)) => (Some(conversation), prompt),
        None => (None, None),
    };
    if held_conversation.is_some_and(|id| {
        id.is_empty()
            || id.len() > 512
            || id.trim() != id
            || id.chars().any(char::is_control)
            || request
                .resume_conversation
                .as_deref()
                .is_some_and(|resume| resume != id)
    }) {
        return Err(AdapterError::Rejected(
            "recorded conversation identity is invalid or conflicts with the open request".into(),
        ));
    }
    adapter.validate_options(handle, &request.options).await?;
    validate_request(
        request,
        format!("agent:{}:open:v1", request.options.slot.as_str()),
    )?;
    validate_fence(authority_fence)?;
    let launch = adapter.runtime.launch(request)?;
    let mut settings = adapter.runtime.open_settings(request)?;
    if settings.recovered_prompt.is_some() {
        return Err(AdapterError::Rejected(
            "runtime supplied prompt recovery authority".into(),
        ));
    }
    settings.recovered_prompt = recovered_prompt;
    // Provider rendering has no authority to add channel tools. Only the
    // configured control-plane source can prove an eligible creation record.
    if settings.context_refresh.is_some()
        || settings.channel_tool.is_some()
        || settings.provider_tool.is_some()
        || settings.access_tool.is_some()
    {
        return Err(AdapterError::Rejected(
            "runtime supplied channel tool authority".into(),
        ));
    }
    if let Some(source) = &adapter.channel_tools {
        settings.channel_tool = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            source.resolve(handle, request, authority_fence),
        )
        .await
        .map_err(|_| AdapterError::Unavailable("channel tool authority timed out".into()))??;
    }
    if let Some(binding) = &settings.channel_tool {
        binding.require_request(handle, request, authority_fence)?;
    }
    settings.provider_tool = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        adapter.resolve_provider_tools(handle, request, authority_fence),
    )
    .await
    .map_err(|_| AdapterError::Unavailable("provider tool authority timed out".into()))??;
    if settings.channel_tool.is_some() && settings.provider_tool.is_some() {
        return Err(AdapterError::Rejected(
            "Agent has conflicting tool authorities".into(),
        ));
    }
    let provider_tool = settings.provider_tool.clone();
    if let Some(source) = &adapter.access_tools {
        settings.access_tool = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            source.resolve(handle, request, authority_fence),
        )
        .await
        .map_err(|_| AdapterError::Unavailable("access tool authority timed out".into()))??;
        if let Some(binding) = &settings.access_tool {
            binding.require_request(handle, request, authority_fence)?;
        }
    }
    let access_tool = settings.access_tool.clone();
    let tool_binding = settings.channel_tool.clone();
    if settings.own_credential != request.options.own_credential {
        return Err(AdapterError::Rejected(
            "agent runtime changed the requested credential ownership mode".into(),
        ));
    }
    if launch.operation_id != request.operation_id
        || launch.request_sha256 != request.request_sha256
        || launch.agent_id != request.options.slot
        || launch.cwd != request.options.working_directory
        || launch.home != request.options.home_directory
    {
        return Err(AdapterError::Rejected(
            "agent launch changed its operation, slot, working directory, or home".into(),
        ));
    }
    settings.context_refresh = adapter
        .bind_context_refresh(handle, request, &settings)
        .await?;
    let (channel, receipt) = adapter
        .backend
        .open_agent(
            handle,
            FencedRequest::new(authority_fence, launch),
            held_conversation
                .map(str::to_owned)
                .or_else(|| request.resume_conversation.clone()),
            settings,
            updates,
        )
        .await?;
    if receipt.authority_fence != authority_fence
        || receipt.receipt.operation_id != request.operation_id
        || receipt.receipt.request_sha256 != request.request_sha256
        || receipt.receipt.agent_id != request.options.slot
        || receipt.receipt.cwd != request.options.working_directory
        || receipt.receipt.home != request.options.home_directory
        || receipt.receipt.conversation_id != channel.conversation_id
        || held_conversation.is_some_and(|id| channel.conversation_id.as_deref() != Some(id))
    {
        return Err(AdapterError::Rejected(
            "runtime returned a receipt for another agent open operation".into(),
        ));
    }
    if let (Some(source), Some(binding)) = (&adapter.channel_tools, tool_binding) {
        let conversation = channel.conversation_id.as_deref().ok_or_else(|| {
            AdapterError::Rejected("channel tool session has no conversation identity".into())
        })?;
        super::super::tool_confirmation::confirm(|| source.confirm_open(&binding, conversation))
            .await?;
    }
    if let (Some(source), Some(binding)) = (&adapter.provider_tools, provider_tool) {
        let conversation = channel.conversation_id.as_deref().ok_or_else(|| {
            AdapterError::Rejected("provider tool session has no conversation identity".into())
        })?;
        super::super::tool_confirmation::confirm(|| source.confirm_open(&binding, conversation))
            .await?;
    }
    if let (Some(source), Some(binding)) = (&adapter.access_tools, access_tool) {
        let conversation = channel.conversation_id.as_deref().ok_or_else(|| {
            AdapterError::Rejected("access tool session has no conversation identity".into())
        })?;
        super::super::tool_confirmation::confirm(|| source.confirm_open(&binding, conversation))
            .await?;
    }
    Ok(channel)
}

pub(super) async fn close_agent(
    adapter: &HostedAdapter,
    handle: &RuntimeHandle,
    authority_fence: u64,
    request: &AgentCloseRequest,
) -> crate::Result<()> {
    validate_fence(authority_fence)?;
    adapter.known_state(handle).await?;
    validate_request(
        request,
        format!("agent:{}:close:v1", request.agent_id.as_str()),
    )?;
    match adapter
        .exchange(
            handle,
            RequestFrame::CloseAgent(FencedRequest::new(authority_fence, request.clone())),
        )
        .await?
    {
        ResponseFrame::CloseAgent(response)
            if response.authority_fence == authority_fence
                && response.receipt.operation_id == request.operation_id
                && response.receipt.request_sha256 == request.request_sha256
                && response.receipt.agent_id == request.agent_id =>
        {
            Ok(())
        }
        ResponseFrame::CloseAgent(_) => Err(AdapterError::Rejected(
            "runtime returned a receipt for another agent close operation".into(),
        )),
        _ => unreachable!("exchange verifies response operation"),
    }
}

trait ReplayRequest {
    fn operation_id(&self) -> &str;
    fn request_sha256(&self) -> &str;
    fn digest_matches(&self) -> bool;
}

macro_rules! replay_request {
    ($($type:ty),+ $(,)?) => {
        $(impl ReplayRequest for $type {
            fn operation_id(&self) -> &str { &self.operation_id }
            fn request_sha256(&self) -> &str { &self.request_sha256 }
            fn digest_matches(&self) -> bool { self.has_valid_sha256() }
        })+
    };
}

replay_request!(
    WorktreeRequest,
    WorktreeRef,
    AgentHomeRequest,
    AgentHomeRemoveRequest,
    AgentOpenRequest,
    AgentCloseRequest,
);

fn validate_request(request: &impl ReplayRequest, expected: String) -> crate::Result<()> {
    if request.operation_id() != expected
        || request.operation_id().len() > 256
        || !valid_sha256(request.request_sha256())
        || !request.digest_matches()
    {
        return Err(AdapterError::Rejected(
            "operation id or request digest is invalid".into(),
        ));
    }
    Ok(())
}

fn validate_fence(authority_fence: u64) -> crate::Result<()> {
    if authority_fence == 0 {
        Err(AdapterError::Rejected(
            "agent mutation authority fence must be nonzero".into(),
        ))
    } else {
        Ok(())
    }
}
