//! Launch reuse discovery over the bounded sandbox-model query ports.

use layer2_core::ids::{SandboxProfileId, WorkspaceProfileId};
use layer2_core::sandbox::store::{
    AgentStore, Page, PageRequest, SandboxStore, SandboxStoreError, WorkspaceStore, MAX_PAGE_SIZE,
};
use layer2_core::sandbox::{
    Agent, AgentOrigin, AgentStatus, Sandbox, SandboxStatus, Workspace, WorkspaceStatus,
};
use layer2_core::trigger_domain::{LaunchContextReader, LaunchResolutionError, TriggerId};
use layer2_core::{seconds_from_rfc3339, SandboxId, SubjectId};

use super::sandbox_mongo::MongoSandboxStore;
mod admitted_agent;

struct Ranked<T> {
    created_at: u64,
    id: String,
    value: T,
}

#[async_trait::async_trait]
impl LaunchContextReader for MongoSandboxStore {
    async fn admitted_trigger_agent(
        &self,
        launch: &layer2_core::trigger_domain::Launch,
    ) -> Result<Option<Agent>, LaunchResolutionError> {
        admitted_agent::read(self, launch).await
    }

    async fn owned_sandbox(
        &self,
        owner: &SubjectId,
        id: &SandboxId,
    ) -> Result<Option<Sandbox>, LaunchResolutionError> {
        let sandbox = SandboxStore::get_sandbox(self, id)
            .await
            .map_err(|error| query_error("read owned Launch Sandbox", error))?;
        Ok(sandbox
            .filter(|sandbox| sandbox.owner == *owner && sandbox.status != SandboxStatus::Retired))
    }

    async fn owned_agent(
        &self,
        owner: &SubjectId,
        agent_id: &layer2_core::AgentId,
    ) -> Result<Option<Agent>, LaunchResolutionError> {
        let agent = AgentStore::get_agent(self, agent_id)
            .await
            .map_err(|error| query_error("read owned Launch Agent", error))?;
        Ok(agent.filter(|agent| agent.owner == *owner))
    }

    async fn newest_matching_sandbox(
        &self,
        owner: &SubjectId,
        profile_id: &SandboxProfileId,
    ) -> Result<Option<Sandbox>, LaunchResolutionError> {
        let mut after = None;
        let mut newest = None;
        loop {
            let page = SandboxStore::list_sandboxes(self, owner, &page_request(after.clone()))
                .await
                .map_err(|error| query_error("list owner Sandboxes", error))?;
            validate_sandbox_page(&page, after.as_deref())?;
            for sandbox in page.items {
                if sandbox.owner != *owner {
                    return Err(corrupt("Sandbox query escaped its owner scope"));
                }
                if sandbox.profile.id == *profile_id && sandbox.status != SandboxStatus::Retired {
                    validate_resource_id(sandbox.id.as_str(), "sbx_", "Sandbox")?;
                    rank_newest(
                        &mut newest,
                        sandbox.created_at.clone(),
                        sandbox.id.as_str().to_owned(),
                        sandbox,
                        "Sandbox",
                    )?;
                }
            }
            let Some(next) = page.next else {
                break;
            };
            after = Some(next);
        }
        Ok(newest.map(|ranked| ranked.value))
    }

    async fn matching_workspace(
        &self,
        owner: &SubjectId,
        sandbox_id: &SandboxId,
        profile_id: &WorkspaceProfileId,
    ) -> Result<Option<Workspace>, LaunchResolutionError> {
        let mut after = None;
        let mut newest = None;
        loop {
            let page =
                WorkspaceStore::list_workspaces(self, sandbox_id, &page_request(after.clone()))
                    .await
                    .map_err(|error| query_error("list Sandbox Workspaces", error))?;
            validate_workspace_page(&page, after.as_deref())?;
            for workspace in page.items {
                if workspace.sandbox_id != *sandbox_id {
                    return Err(corrupt("Workspace query escaped its Sandbox scope"));
                }
                if workspace.owner != *owner {
                    return Err(corrupt("Workspace owner disagrees with its Sandbox owner"));
                }
                if workspace.profile.id == *profile_id
                    && workspace.status != WorkspaceStatus::Removed
                {
                    validate_reusable_workspace(&workspace)?;
                    rank_newest(
                        &mut newest,
                        workspace.created_at.clone(),
                        workspace.id.as_str().to_owned(),
                        workspace,
                        "Workspace",
                    )?;
                }
            }
            let Some(next) = page.next else {
                break;
            };
            after = Some(next);
        }
        Ok(newest.map(|ranked| ranked.value))
    }

    async fn newest_live_trigger_agent(
        &self,
        owner: &SubjectId,
        trigger_id: &TriggerId,
    ) -> Result<Option<Agent>, LaunchResolutionError> {
        let mut after = None;
        let mut newest = None;
        loop {
            let page = AgentStore::list_agents(self, owner, &page_request(after.clone()))
                .await
                .map_err(|error| query_error("list owner Agents", error))?;
            validate_agent_page(&page, after.as_deref())?;
            for agent in page.items {
                if agent.owner != *owner {
                    return Err(corrupt("Agent query escaped its owner scope"));
                }
                if agent.status.is_live() && trigger_origin_matches(&agent.origin, trigger_id)? {
                    validate_live_agent(&agent)?;
                    agent.validate_conversation().map_err(|error| {
                        corrupt(&format!("matching Agent conversation is invalid: {error}"))
                    })?;
                    rank_newest(
                        &mut newest,
                        agent.created_at.clone(),
                        agent.id.as_str().to_owned(),
                        agent,
                        "Agent",
                    )?;
                }
            }
            let Some(next) = page.next else {
                break;
            };
            after = Some(next);
        }
        Ok(newest.map(|ranked| ranked.value))
    }
}

fn page_request(cursor: Option<String>) -> PageRequest {
    PageRequest {
        limit: MAX_PAGE_SIZE,
        cursor,
    }
}

fn validate_sandbox_page(
    page: &Page<Sandbox>,
    after: Option<&str>,
) -> Result<(), LaunchResolutionError> {
    let ids = page
        .items
        .iter()
        .map(|sandbox| sandbox.id.as_str())
        .collect::<Vec<_>>();
    validate_page_order(&ids, page.next.as_deref(), after, "Sandbox")
}

fn validate_workspace_page(
    page: &Page<Workspace>,
    after: Option<&str>,
) -> Result<(), LaunchResolutionError> {
    let ids = page
        .items
        .iter()
        .map(|workspace| workspace.id.as_str())
        .collect::<Vec<_>>();
    validate_page_order(&ids, page.next.as_deref(), after, "Workspace")
}

fn validate_agent_page(
    page: &Page<Agent>,
    after: Option<&str>,
) -> Result<(), LaunchResolutionError> {
    let ids = page
        .items
        .iter()
        .map(|agent| agent.id.as_str())
        .collect::<Vec<_>>();
    validate_page_order(&ids, page.next.as_deref(), after, "Agent")
}

fn validate_page_order(
    ids: &[&str],
    next: Option<&str>,
    after: Option<&str>,
    resource: &str,
) -> Result<(), LaunchResolutionError> {
    if ids.len() > usize::from(MAX_PAGE_SIZE) {
        return Err(corrupt(&format!(
            "{resource} query exceeded its page bound"
        )));
    }
    let mut previous = after;
    for id in ids {
        if previous.is_some_and(|previous| *id <= previous) {
            return Err(corrupt(&format!(
                "{resource} query returned non-deterministic ordering"
            )));
        }
        previous = Some(*id);
    }
    if next.is_some() && next != ids.last().copied() {
        return Err(corrupt(&format!(
            "{resource} query returned an invalid continuation cursor"
        )));
    }
    Ok(())
}

fn rank_newest<T>(
    newest: &mut Option<Ranked<T>>,
    created_at: String,
    id: String,
    value: T,
    resource: &str,
) -> Result<(), LaunchResolutionError> {
    let created_at = seconds_from_rfc3339(&created_at)
        .ok_or_else(|| corrupt(&format!("matching {resource} has an invalid creation time")))?;
    let replace = newest.as_ref().is_none_or(|current| {
        (created_at, id.as_str()) > (current.created_at, current.id.as_str())
    });
    if replace {
        *newest = Some(Ranked {
            created_at,
            id,
            value,
        });
    }
    Ok(())
}

fn validate_resource_id(
    value: &str,
    prefix: &str,
    resource: &str,
) -> Result<(), LaunchResolutionError> {
    if !value.starts_with(prefix) || value.len() == prefix.len() {
        return Err(corrupt(&format!(
            "matching {resource} has an invalid resource ID"
        )));
    }
    Ok(())
}

fn validate_reusable_workspace(workspace: &Workspace) -> Result<(), LaunchResolutionError> {
    validate_resource_id(workspace.id.as_str(), "wks_", "Workspace")?;
    workspace
        .validate_origin()
        .map_err(|error| corrupt(&format!("matching Workspace is invalid: {error}")))?;

    let failure_is_consistent =
        (workspace.status == WorkspaceStatus::Failed) == workspace.failed.is_some();
    let ready_is_consistent = match workspace.status {
        WorkspaceStatus::Preparing => workspace.ready_at.is_none(),
        WorkspaceStatus::Ready => {
            workspace.ready_at.is_some() && workspace.resolved_commit.is_some()
        }
        WorkspaceStatus::Failed | WorkspaceStatus::Removing => true,
        WorkspaceStatus::Removed => false,
    };
    let ready_time_is_valid = workspace
        .ready_at
        .as_deref()
        .is_none_or(|value| seconds_from_rfc3339(value).is_some());
    if !failure_is_consistent
        || !ready_is_consistent
        || !ready_time_is_valid
        || workspace.removed_at.is_some()
    {
        return Err(corrupt("matching Workspace has an invalid lifecycle"));
    }
    Ok(())
}

fn validate_live_agent(agent: &Agent) -> Result<(), LaunchResolutionError> {
    let ready_is_consistent = match agent.status {
        AgentStatus::Starting => agent.ready_at.is_none(),
        AgentStatus::Ready
        | AgentStatus::Working
        | AgentStatus::WaitingApproval
        | AgentStatus::Paused => agent.ready_at.is_some(),
        AgentStatus::Stopped | AgentStatus::Deleted | AgentStatus::Failed => false,
    };
    let ready_time_is_valid = agent
        .ready_at
        .as_deref()
        .is_none_or(|value| seconds_from_rfc3339(value).is_some());
    if !ready_is_consistent
        || !ready_time_is_valid
        || agent.failure.is_some()
        || agent.stopped_at.is_some()
        || agent.stop_reason.is_some()
        || agent.deleted_at.is_some()
    {
        return Err(corrupt("matching Agent has an invalid lifecycle"));
    }
    Ok(())
}

fn trigger_origin_matches(
    origin: &AgentOrigin,
    expected: &TriggerId,
) -> Result<bool, LaunchResolutionError> {
    let AgentOrigin::Trigger {
        trigger_id,
        launch_id,
    } = origin
    else {
        return Ok(false);
    };
    if trigger_id != expected.as_str() {
        return Ok(false);
    }
    if launch_id.trim().is_empty()
        || launch_id.len() > 128
        || launch_id.chars().any(char::is_control)
    {
        return Err(corrupt(
            "matching Agent has an invalid Trigger launch origin",
        ));
    }
    Ok(true)
}

fn query_error(context: &str, error: SandboxStoreError) -> LaunchResolutionError {
    match error {
        SandboxStoreError::Unavailable(detail) => {
            LaunchResolutionError::Unavailable(format!("{context}: {detail}"))
        }
        SandboxStoreError::Corrupt(detail) => {
            LaunchResolutionError::Corrupt(format!("{context}: {detail}"))
        }
        error => LaunchResolutionError::Corrupt(format!("{context} failed unexpectedly: {error}")),
    }
}

fn corrupt(detail: &str) -> LaunchResolutionError {
    LaunchResolutionError::Corrupt(detail.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: &str = "2026-09-05T00:00:00Z";
    const LATER: &str = "2026-09-05T00:01:00Z";

    #[test]
    fn newest_rank_uses_creation_time_then_id() {
        let mut newest = None;
        rank_newest(&mut newest, LATER.into(), "a".into(), "later-a", "test").unwrap();
        rank_newest(&mut newest, NOW.into(), "z".into(), "older-z", "test").unwrap();
        rank_newest(&mut newest, LATER.into(), "b".into(), "later-b", "test").unwrap();
        assert_eq!(newest.unwrap().value, "later-b");

        let mut invalid = None;
        assert!(rank_newest(&mut invalid, "later".into(), "c".into(), (), "test").is_err());
    }

    #[test]
    fn page_order_and_cursor_are_fail_closed() {
        assert!(validate_page_order(&["a", "b"], Some("b"), None, "test").is_ok());
        assert!(validate_page_order(&["b"], None, Some("a"), "test").is_ok());
        assert!(validate_page_order(&["b", "a"], None, None, "test").is_err());
        assert!(validate_page_order(&["a"], None, Some("a"), "test").is_err());
        assert!(validate_page_order(&["a"], Some("different"), None, "test").is_err());
        assert!(validate_page_order(&[], Some("a"), None, "test").is_err());
        let oversized = vec!["a"; usize::from(MAX_PAGE_SIZE) + 1];
        assert!(validate_page_order(&oversized, None, None, "test").is_err());
    }

    #[test]
    fn resource_ids_and_store_errors_fail_closed() {
        assert!(validate_resource_id("sbx_valid", "sbx_", "Sandbox").is_ok());
        assert!(validate_resource_id("invalid", "sbx_", "Sandbox").is_err());
        assert!(validate_resource_id("wks_", "wks_", "Workspace").is_err());

        assert_eq!(
            query_error("query", SandboxStoreError::Unavailable("offline".into())),
            LaunchResolutionError::Unavailable("query: offline".into())
        );
        assert_eq!(
            query_error("query", SandboxStoreError::Corrupt("broken".into())),
            LaunchResolutionError::Corrupt("query: broken".into())
        );
        assert!(matches!(
            query_error("query", SandboxStoreError::NotFound),
            LaunchResolutionError::Corrupt(_)
        ));
    }

    #[test]
    fn trigger_origin_is_exact_and_validated() {
        let expected = TriggerId("trg_expected".into());
        assert!(trigger_origin_matches(
            &AgentOrigin::Trigger {
                trigger_id: expected.as_str().into(),
                launch_id: "lch_prior".into(),
            },
            &expected,
        )
        .unwrap());
        assert!(!trigger_origin_matches(&AgentOrigin::Manual, &expected).unwrap());
        assert!(!trigger_origin_matches(
            &AgentOrigin::Trigger {
                trigger_id: "trg_other".into(),
                launch_id: String::new(),
            },
            &expected,
        )
        .unwrap());
        assert!(trigger_origin_matches(
            &AgentOrigin::Trigger {
                trigger_id: expected.as_str().into(),
                launch_id: String::new(),
            },
            &expected,
        )
        .is_err());
    }
}
