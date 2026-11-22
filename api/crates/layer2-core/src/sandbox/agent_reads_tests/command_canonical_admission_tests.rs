use super::*;

#[tokio::test]
async fn task_acceptance_is_opt_in_for_the_personal_owner_and_replays_the_exact_contract() {
    let (store, disabled, authority, _) = setup(Authority::new()).await;
    let access = Arc::new(ResourceAccessService::new(store.clone(), authority.clone()));
    let selected = AgentCommandService::new(store, access, authority)
        .with_task_experience(Arc::new(
        serde_json::from_value(serde_json::json!({
            "mode": "accounts", "accounts": ["owner"], "capabilities": ["task_acceptance_admission"]
        }))
        .unwrap(),
    ));
    let owner = context("owner");
    let mut intent = submission("scoped-task");
    let AgentCommand::Message { input, .. } = &mut intent.command else {
        unreachable!()
    };
    input.task_acceptance = Some(serde_json::from_value(serde_json::json!({
        "version": 1,
        "action": {"type": "declare", "requirements": [
            {"id": "report", "required": true, "evidence": {"kind": "plan_step", "title": "Write report"}}
        ]}
    })).unwrap());
    let refusal = disabled
        .admit(
            &owner,
            "selected-credential",
            CommandSubmissionSource::Api,
            intent.clone(),
        )
        .await;
    assert!(matches!(
        refusal,
        Err(CommandAdmissionError::Access(ResourceAccessError::Access(
            AccessError::Forbidden
        )))
    ));
    let admitted = selected
        .admit(
            &owner,
            "selected-credential",
            CommandSubmissionSource::Api,
            intent.clone(),
        )
        .await
        .unwrap();
    let retained = disabled
        .admit(
            &owner,
            "selected-credential",
            CommandSubmissionSource::Api,
            intent,
        )
        .await
        .unwrap();
    assert_eq!(admitted, retained);
    assert!(selected
        .admit(
            &owner,
            "selected-credential",
            CommandSubmissionSource::Api,
            submission("scoped-task")
        )
        .await
        .is_err());
}

#[tokio::test]
async fn admission_assigns_attribution_and_retries_preserve_receipt_after_agent_revision_changes() {
    let (store, service, authority, agent) = setup(Authority::new()).await;
    let before_log = store.agent_log(&agent.id, 0, 20).await.unwrap();
    let owner = context("owner");
    let mut retry = submission("one");
    retry.command = AgentCommand::Message {
        input: AgentInput {
            task_acceptance: None,
            content: vec![
                AgentContent::Text {
                    text: "private ".into(),
                },
                AgentContent::Text {
                    text: "input".into(),
                },
            ],
        },
        order: AgentInputOrder::Append,
        metadata: Default::default(),
    };
    let (a, b) = tokio::join!(
        service.admit(
            &owner,
            "selected-credential",
            CommandSubmissionSource::Api,
            submission("one")
        ),
        service.admit(
            &owner,
            "selected-credential",
            CommandSubmissionSource::UserInterface,
            retry
        ),
    );
    let a = a.unwrap();
    assert_eq!(a, b.unwrap());
    assert_eq!(a.status, AgentResponseStatus::Queued);
    let record = store
        .get_command(&a.receipt.command_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.receipt.envelope.actor, owner.actor);
    assert_eq!(
        record.receipt.envelope.owner,
        AgentOwner::Person {
            subject: owner.caller.subject.clone()
        }
    );
    assert_eq!(
        record.receipt.envelope.source.reference,
        record.receipt.envelope.command_id.to_string()
    );
    assert!(!serde_json::to_string(&record)
        .unwrap()
        .contains("selected-credential"));
    let mut changed = agent.clone();
    changed.revision += 1;
    changed.title = Some("updated without a new command".into());
    store.save_agent(agent.revision, changed).await.unwrap();
    let mut retry = submission("one");
    retry.expected_agent_revision = Some(agent.revision);
    assert_eq!(
        service
            .admit(
                &owner,
                "selected-credential",
                CommandSubmissionSource::Api,
                retry
            )
            .await
            .unwrap(),
        a
    );
    let mut stale_new = submission("two");
    stale_new.expected_agent_revision = Some(agent.revision);
    assert!(matches!(
        service
            .admit(
                &owner,
                "selected-credential",
                CommandSubmissionSource::Api,
                stale_new
            )
            .await,
        Err(CommandAdmissionError::Ledger(CommandLedgerError::Conflict))
    ));
    let mut conflict = submission("one");
    conflict.command = AgentCommand::message("different input");
    assert!(matches!(
        service
            .admit(
                &owner,
                "selected-credential",
                CommandSubmissionSource::Api,
                conflict
            )
            .await,
        Err(CommandAdmissionError::Ledger(
            CommandLedgerError::IdempotencyConflict
        ))
    ));
    assert_eq!(
        service
            .response_status(&owner, None, a.receipt.response_id.as_ref().unwrap())
            .await
            .unwrap(),
        a
    );
    assert_eq!(
        service
            .command_status(&owner, None, &a.receipt.command_id)
            .await
            .unwrap(),
        a
    );
    assert_eq!(store.agent_log(&agent.id, 0, 20).await.unwrap(), before_log);
    assert_eq!(
        store
            .command_work_page(&PageRequest::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    assert!(authority.calls.load(Ordering::SeqCst) >= 2);
}
