use super::*;
use crate::sandbox::AgentSettings;

#[tokio::test]
async fn explicit_model_admission_uses_selected_settings_and_preserves_profile() {
    let store = Arc::new(InMemorySandboxStore::default());
    store
        .create_sandbox(sandbox("sandbox", "owner"))
        .await
        .unwrap();
    store
        .create_workspace(workspace("workspace", "sandbox", "owner"))
        .await
        .unwrap();
    let mut agent = agent(
        "agent",
        "worktree",
        "workspace",
        "sandbox",
        "owner",
        AgentStatus::Starting,
    );
    agent.conversation.settings = Some(AgentSettings {
        model: "selected-alternate".into(),
        ..agent.settings()
    });
    let agent = insert_agent(
        &store,
        agent,
        worktree("worktree", "workspace", "sandbox", "agent", "owner"),
    )
    .await;
    let authority = Arc::new(Authority::new());
    let access = Arc::new(ResourceAccessService::new(store.clone(), authority.clone()));
    let service = AgentCommandService::new(store.clone(), access, authority);
    let mut input = submission("selected-model");
    input.requested_model = Some("selected-alternate".into());
    let receipt = service
        .admit(
            &context("owner"),
            "selected-credential",
            CommandSubmissionSource::Api,
            input,
        )
        .await
        .unwrap();
    assert_eq!(receipt.status, AgentResponseStatus::Queued);
    assert_eq!(
        store.get_agent(&agent.id).await.unwrap().unwrap().profile,
        agent.profile
    );

    let mut stale = submission("creation-model");
    stale.requested_model = agent
        .profile
        .content
        .model
        .clone()
        .or(Some("deployment-default".into()));
    assert!(matches!(
        service
            .admit(
                &context("owner"),
                "selected-credential",
                CommandSubmissionSource::Api,
                stale
            )
            .await,
        Err(CommandAdmissionError::ModelMismatch)
    ));
    assert_eq!(
        store
            .command_work_page(&PageRequest::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
}
