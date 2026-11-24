use super::*;

#[tokio::test]
async fn a_replacement_write_grant_cannot_authorize_a_recovered_edit() {
    let (files, access, owner) = fixture().await;
    let agent_id = agent().id;
    let viewer = context("viewer");
    let grant = |token: String, key: &str| CreateAgentShareLink {
        agent_id: agent_id.clone(),
        audience: AgentShareAudience::Public,
        mode: ShareMode::ViewWrite,
        token,
        idempotency_key: key.into(),
    };
    let original_token = "a".repeat(64);
    let original = access
        .create_agent_share_link(&owner, None, grant(original_token.clone(), "original"))
        .await
        .unwrap();
    access
        .redeem_agent_share_link(&viewer, None, &original.share.id, &original_token)
        .await
        .unwrap();
    let target = files
        .authorize_runtime_operation(
            &viewer,
            None,
            &agent_id,
            ResourceCapability::OperateAgent,
            AccessPurpose::Mutation {
                started_at: std::time::SystemTime::now(),
            },
            true,
        )
        .await
        .unwrap()
        .unwrap();
    let current = access
        .read_share(
            &owner,
            None,
            &ResourceSelector::Agent {
                agent_id: agent_id.clone(),
            },
            &original.share.id,
        )
        .await
        .unwrap();
    access
        .change_share(
            &owner,
            None,
            ShareCommand {
                selector: ResourceSelector::Agent {
                    agent_id: agent_id.clone(),
                },
                change: ShareChange::Revoke {
                    share_id: current.id,
                },
                expected_share_revision: Some(current.revision),
                idempotency_key: "revoke-original".into(),
            },
        )
        .await
        .unwrap();
    let replacement_token = "b".repeat(64);
    let replacement = access
        .create_agent_share_link(
            &owner,
            None,
            grant(replacement_token.clone(), "replacement"),
        )
        .await
        .unwrap();
    access
        .redeem_agent_share_link(&viewer, None, &replacement.share.id, &replacement_token)
        .await
        .unwrap();
    assert_ne!(original.share.id, replacement.share.id);
    assert!(files
        .authorize_runtime_operation(
            &viewer,
            None,
            &agent_id,
            ResourceCapability::OperateAgent,
            AccessPurpose::Mutation {
                started_at: std::time::SystemTime::now()
            },
            true,
        )
        .await
        .unwrap()
        .is_some());
    assert!(matches!(
        files
            .write(
                &viewer.caller,
                Some(&viewer),
                None,
                &agent_id,
                "note",
                Some("0".repeat(64)),
                "aGk=".into(),
                false,
                Some(&target),
            )
            .await,
        Err(AgentFileError::Conflict)
    ));
}
