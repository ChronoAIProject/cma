use super::*;

#[tokio::test]
async fn public_control_capability_refusal_is_distinct_and_replay_precedes_it() {
    let (store, service, _, mut request) = fixture().await;
    let accepted = service
        .admit_response_control(
            &context("owner"),
            "selected-credential",
            CommandSubmissionSource::Api,
            request.clone(),
        )
        .await
        .unwrap();
    let mut agent = store.get_agent(&request.agent_id).await.unwrap().unwrap();
    let previous = agent.revision;
    agent.capabilities = RuntimeCapabilities::new([RuntimeCapability::Conversation]);
    agent.revision += 1;
    store.save_agent(previous, agent.clone()).await.unwrap();
    assert_eq!(
        service
            .admit_response_control(
                &context("owner"),
                "selected-credential",
                CommandSubmissionSource::Api,
                request.clone()
            )
            .await
            .unwrap(),
        accepted
    );
    request.idempotency_key = "new-unsupported".into();
    request.expected_agent_revision = agent.revision;
    assert!(matches!(
        service
            .admit_response_control(
                &context("owner"),
                "selected-credential",
                CommandSubmissionSource::Api,
                request.clone()
            )
            .await,
        Err(TargetedControlError::CapabilityUnsupported)
    ));
    request.target.turn_sequence += 1;
    assert!(matches!(
        service
            .admit_response_control(
                &context("owner"),
                "selected-credential",
                CommandSubmissionSource::Api,
                request
            )
            .await,
        Err(TargetedControlError::Admission(
            CommandAdmissionError::Ledger(CommandLedgerError::NotAdmissible)
        ))
    ));
}

#[tokio::test]
async fn control_observation_is_safe_current_history_and_not_operation_authority() {
    let (store, service, authority, request) = fixture().await;
    share(&store, None).await;
    let observed = service
        .observe_response_controls(
            &context("reader"),
            Some("selected-credential"),
            &request.agent_id,
            &request.target.response_id,
        )
        .await
        .unwrap();
    assert_eq!(observed.target.as_ref(), Some(&request.target));
    assert!(observed.target_active && observed.runtime_support.cancel);
    assert!(service
        .admit_response_control(
            &context("reader"),
            "selected-credential",
            CommandSubmissionSource::Api,
            request.clone()
        )
        .await
        .is_err());
    // This fixture authority grants its configured role to every queried
    // subject. Revoke that membership before asserting a nonmember read fails.
    *authority.membership.lock().unwrap() = Err(MembershipError::NotMember);
    for (caller, agent) in [
        (context("unrelated"), request.agent_id.clone()),
        (context("owner"), AgentId("wrong-agent".into())),
    ] {
        assert!(service
            .observe_response_controls(
                &caller,
                Some("selected-credential"),
                &agent,
                &request.target.response_id
            )
            .await
            .is_err());
    }
    let mut agent = store.get_agent(&request.agent_id).await.unwrap().unwrap();
    let previous = agent.revision;
    agent.created_at = "2099-09-05T00:00:00Z".into();
    agent.revision += 1;
    store.save_agent(previous, agent).await.unwrap();
    assert!(matches!(
        service
            .observe_response_controls(
                &context("owner"),
                Some("selected-credential"),
                &request.agent_id,
                &request.target.response_id
            )
            .await,
        Err(CommandAdmissionError::Ledger(CommandLedgerError::NotFound))
    ));
}
