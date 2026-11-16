use super::*;

#[tokio::test]
async fn denied_or_revoked_source_never_contacts_owner() {
    let f = fixture(None).await;
    assert!(f
        .service
        .admit(
            &context("other"),
            "fixture-credential",
            CommandSubmissionSource::Api,
            submission("wrong-principal")
        )
        .await
        .is_err());
    assert!(matches!(
        f.service
            .admit(
                &context("owner-a"),
                "revoked-fixture",
                CommandSubmissionSource::Api,
                submission("wrong-credential")
            )
            .await,
        Err(CommandAdmissionError::Authority(
            CommandAuthorizationError::Revoked
        ))
    ));
    assert!(f.router.mutations.lock().unwrap().is_empty());
    assert_eq!(f.ingress.0.load(Ordering::SeqCst), 0);
    assert!(records(&f).await.is_empty());
}

#[tokio::test]
async fn uncertain_admission_never_retries_or_falls_back_on_ingress() {
    for failure in ["unavailable", "reply_lost", "wrong_agent"] {
        let f = fixture(Some(failure)).await;
        assert!(matches!(
            admit(&f, submission("uncertain")).await,
            Err(CommandAdmissionError::Ledger(
                CommandLedgerError::Unavailable
            ))
        ));
        assert_eq!(f.router.mutations.lock().unwrap().len(), 1);
        assert_eq!(f.ingress.0.load(Ordering::SeqCst), 0);
        assert_eq!(
            records(&f).await.len(),
            usize::from(failure != "unavailable")
        );
    }
}

#[tokio::test]
async fn receiving_execution_requires_exact_positive_unrevoked_authority() {
    let f = fixture(None).await;
    // Keep the caller alive for the whole owner invocation.
    let caller = context("owner-a");
    let mutation = || OwnerCommandMutation::Admit {
        submission: submission("owner-fence"),
        binding_policy: None,
    };
    assert!(f
        .router
        .owner
        .execute_owner_mutation(
            &caller,
            "fixture-credential",
            CommandSubmissionSource::Api,
            mutation()
        )
        .await
        .is_err());
    let wrong_protocol = OwnerAuthority::new(
        OwnershipToken {
            agent_id: AgentId(ID.into()),
            instance_id: "am-owner".into(),
            epoch: 2,
            protocol: PROTOCOL_VERSION + 1,
        },
        tokio::time::Instant::now() + Duration::from_secs(30),
    );
    for authority in [ownership(ID, 0), ownership("agt_other", 2), wrong_protocol] {
        let result = authority
            .run(f.router.owner.execute_owner_mutation(
                &caller,
                "fixture-credential",
                CommandSubmissionSource::Api,
                mutation(),
            ))
            .await
            .unwrap();
        assert!(result.is_err());
    }
    let revoked = ownership(ID, 1);
    revoked.revoke();
    assert!(revoked
        .run(f.router.owner.execute_owner_mutation(
            &caller,
            "fixture-credential",
            CommandSubmissionSource::Api,
            mutation()
        ))
        .await
        .is_err());
    assert!(records(&f).await.is_empty());
}
