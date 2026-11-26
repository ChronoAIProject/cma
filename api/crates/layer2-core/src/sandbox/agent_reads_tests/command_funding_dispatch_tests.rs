use super::*;
use crate::sandbox::model_credit::tests::BeginReply;

#[tokio::test]
async fn model_credit_uncertain_begin_retains_the_operation_and_command_for_recovery() {
    for reply in [
        BeginReply::Error(FundingError::Unavailable),
        BeginReply::Error(FundingError::CheckpointChanged),
        BeginReply::UnchangedVersion,
    ] {
        let (store, access, authority, agent) = execution_fixture(MAX_MEMBERSHIP_AGE).await;
        let binding = payer(credential(CredentialSource::Managed));
        let ledger = Arc::new(Ledger::default());
        ledger.lock().unwrap().add(&binding, 5);
        let gateway = gateway(false);
        let funding = AgentTurnFunding::new(ledger.clone(), gateway.clone());
        let access = Arc::new(access);
        let commands = AgentCommandService::new(store.clone(), access.clone(), authority)
            .with_model_credit_required(true)
            .with_model_funding(Some(funding.clone()));
        let caller = context("owner");
        let accepted = commands
            .admit(
                &caller,
                "selected-credential",
                CommandSubmissionSource::Api,
                submission("uncertain-begin"),
            )
            .await
            .unwrap();
        let now = SystemTime::now();
        let command = store
            .claim_next_command(CommandClaimRequest {
                agent_id: agent.id.clone(),
                holder: "worker".into(),
                now: crate::rfc3339_at(now),
                expires_at: crate::rfc3339_at(now + Duration::from_secs(30)),
            })
            .await
            .unwrap()
            .unwrap();
        let authorization = access
            .authorize_command_execution(&caller, "selected-credential", &command.receipt.envelope)
            .await
            .unwrap();
        let (registry, legacy_sends) = runtime(store.clone(), None, 0);
        let operations = AgentOperationService::new(store.clone(), registry)
            .with_model_credit_required(true)
            .with_model_funding(Some(funding));
        ledger.lock().unwrap().begin_reply = Some(reply);
        let outcome = operations
            .execute_resource_claim(&authorization, &command)
            .await;
        let current = store.get_agent(&agent.id).await.unwrap().unwrap();
        assert_eq!(
            current.conversation.operation.as_ref().unwrap().state,
            AgentOperationDelivery::RecoveryRequired,
            "{reply:?}: {outcome:?}"
        );
        assert!(matches!(
            outcome,
            Err(AgentOperationError::DeliveryAmbiguous { .. })
        ));
        assert!(current.conversation.active_turn.is_some());
        let id = &accepted.receipt.command_id;
        let retained = store.get_command(id).await.unwrap().unwrap();
        assert_eq!(retained.receipt, command.receipt);
        assert!(retained.work.dispatch.is_some());

        let successor =
            AgentCommandWorker::new(store.clone(), Arc::new(NoRedispatch), "successor".into())
                .unwrap()
                .with_model_funding(Some(AgentTurnFunding::new(ledger.clone(), gateway.clone())));
        let page = PageRequest::new(1, None).unwrap();
        for _ in 0..2 {
            assert!(!successor.run_scheduled(id).await.unwrap().is_zero());
            assert!(successor
                .discover_scheduled(&page)
                .await
                .unwrap()
                .page
                .items
                .iter()
                .any(|item| &item.id == id));
            assert_eq!(*ledger.lock().unwrap().available(&binding), 0);
            assert_eq!(gateway.calls.load(Ordering::SeqCst), 0);
            assert_eq!(legacy_sends.load(Ordering::SeqCst), 0);
        }
    }
}
