use super::*;
mod direct_erasure;
mod fixture;
mod races;
mod standing;
mod uncertainty;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn ticket_continuation_and_release_write_conflict_with_freeze() {
    use fixture::{context, digest, generation, id, Fixture};
    use layer2_core::resource_access::ResourceCapability;
    for admission in ["ticket", "continuation", "release"] {
        let fixture = Fixture::new().await;
        let ticket = fixture.started_ticket(admission).await;
        let authority = fixture
            .coordinator
            .authorize_execution(
                &context("recipient"),
                Some("synthetic"),
                &fixture.target,
                ResourceCapability::Interact,
                Some(&ticket.basis),
            )
            .await
            .unwrap();
        let continuation = wire::ContinuationV1 {
            protocol: 1,
            execution: ticket.execution.clone(),
            registration: ticket.registration.clone(),
            basis: ticket.basis.clone(),
            manager: ticket.manager.clone(),
            sequence: generation(2),
            challenge: wire::ReceiverChallengeV1 {
                protocol: 1,
                receiver: ticket.execution.receiver.clone(),
                nonce: id("continue"),
                created_tick_ms: 1_100,
                deadline_tick_ms: 4_100,
            },
        };
        fixture
            .coordinator
            .continue_execution(&authority, &continuation)
            .await
            .unwrap();
        let read = fixture
            .coordinator
            .authorize_execution(
                &context("recipient"),
                Some("synthetic"),
                &fixture.target,
                ResourceCapability::ViewHistory,
                None,
            )
            .await
            .unwrap();
        let release = wire::ReleaseKey {
            response_id: id("response"),
            content_fingerprint: digest('f'),
            receiver: wire::ReceiverKey {
                kind: wire::ReceiverKind::ReleaseProducer,
                instance: id("api"),
                incarnation: digest('e'),
                boot: id("api-boot"),
            },
        };
        let member = fixture
            .coordinator
            .register_release(&read, &release)
            .await
            .unwrap();
        let permit = wire::ReleasePermitV1 {
            protocol: 1,
            release: release.clone(),
            basis: read.basis().clone(),
            registration: member.registration.clone(),
            sequence: generation(1),
            challenge: wire::ReceiverChallengeV1 {
                protocol: 1,
                receiver: release.receiver.clone(),
                nonce: id("release"),
                created_tick_ms: 100,
                deadline_tick_ms: 3_100,
            },
        };
        fixture
            .coordinator
            .issue_release(&read, &permit)
            .await
            .unwrap();
        let mut session = fixture.client.start_session().await.unwrap();
        start_transaction(&mut session).await.unwrap();
        fixture
            .store
            .collections
            .grants
            .find_one(doc! {"_id":ticket.basis.family().unwrap().origin.as_str()})
            .session(&mut session)
            .await
            .unwrap();
        let operation = fixture.freeze().await;
        let result = match admission {
            "ticket" => fixture
                .store
                .persist_ticket(&authority, &ticket, &mut session)
                .await
                .map(|_| ()),
            "continuation" => fixture
                .store
                .persist_continuation(&authority, &continuation, &mut session)
                .await
                .map(|_| ()),
            _ => fixture
                .store
                .persist_release(&read, &permit, &mut session)
                .await
                .map(|_| ()),
        };
        assert!(
            matches!(result, Err(TxnError::Driver(ref error)) if is_transient(error)),
            "{admission}: {result:?}"
        );
        session.abort_transaction().await.unwrap();
        let result = match admission {
            "ticket" => fixture
                .store
                .issue_ticket(&authority, &ticket)
                .await
                .map(|_| ()),
            "continuation" => fixture
                .store
                .continue_execution(&authority, &continuation)
                .await
                .map(|_| ()),
            _ => fixture
                .store
                .issue_release(&read, &permit)
                .await
                .map(|_| ()),
        };
        assert!(
            matches!(result, Err(ExecutionError::Closing { .. })),
            "{admission}: {result:?}"
        );
        let execution = fixture
            .store
            .record(&ticket.registration)
            .await
            .unwrap()
            .unwrap();
        fixture.complete(&operation, &execution).await;
        assert!(fixture
            .store
            .finish_revocation(&operation.key.operation_id)
            .await
            .is_err());
        let member = fixture
            .store
            .record(&member.registration)
            .await
            .unwrap()
            .unwrap();
        fixture.complete(&operation, &member).await;
        fixture
            .store
            .finish_revocation(&operation.key.operation_id)
            .await
            .unwrap();
        fixture.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn wrong_grant_runtime_boot_turn_or_agent_evidence_never_finalizes() {
    use fixture::{digest, generation, id, Fixture};
    let fixture = Fixture::new().await;
    let member = fixture.register("recipient", "exact-member").await;
    let operation = fixture.freeze().await;
    let request = member.barrier_request(&operation).unwrap();
    for mismatch in 0..6 {
        let mut wrong = request.clone();
        let wire::MemberKey::Execution { execution } = &mut wrong.member else {
            panic!()
        };
        match mismatch {
            0 => wrong.revocation.family.generation = generation(2),
            1 => execution.receiver.incarnation = digest('f'),
            2 => execution.receiver.boot = id("other-boot"),
            3 => execution.turn_id = id("other-turn"),
            4 => execution.agent_id = id("other-agent"),
            _ => wrong.revocation.family.origin = digest('f'),
        }
        let barrier = wire::BarrierReceiptV1 {
            protocol: 1,
            request: wrong,
            barrier_sequence: generation(2),
            durable_watermark: operation.key.watermark,
        };
        assert!(fixture
            .coordinator
            .observe_barrier(member.member.receiver(), &barrier)
            .await
            .is_err());
        assert!(fixture
            .store
            .finish_revocation(&operation.key.operation_id)
            .await
            .is_err());
    }
    fixture.complete(&operation, &member).await;
    fixture
        .store
        .finish_revocation(&operation.key.operation_id)
        .await
        .unwrap();
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn binding_authority_survives_unrelated_freeze_but_rechecks_key_rotation() {
    use fixture::Fixture;
    use layer2_core::resource_access::{
        CallerContext, CredentialContext, CredentialSource, ResourceActor, ResourceCapability,
    };
    use layer2_core::sandbox::binding::{BindingAccessService, BindingPrincipal, BindingStore};
    let fixture = Fixture::new().await;
    let binding = fixture.borrowed_binding().await;
    let bindings = BindingAccessService::new(Arc::new(fixture.sandbox.clone()));
    let principal = bindings
        .authenticate(&binding.key_sha256)
        .await
        .unwrap()
        .unwrap();
    let context = |principal: &BindingPrincipal| CallerContext {
        credential_source: CredentialSource::Binding,
        caller: principal.caller(),
        credential: CredentialContext {
            issuer: "binding:local".into(),
            subject: principal.owner().clone(),
            credential_id: principal.key_sha256().into(),
        },
        actor: ResourceActor::Api {
            subject: principal.owner().clone(),
            delegation_id: None,
        },
        memberships: vec![],
    };
    let policy = ResourceExecutionPolicy::enforce(fixture.store.clone(), true)
        .await
        .unwrap();
    let coordinator = ResourceExecutionCoordinator::new(
        fixture.access.clone().with_bindings(bindings.clone()),
        policy,
    );
    let authority = coordinator
        .authorize_execution(
            &context(&principal),
            None,
            &fixture.target,
            ResourceCapability::Interact,
            None,
        )
        .await
        .unwrap();
    assert!(matches!(
        authority.basis(),
        wire::ExecutionBasisV1::Independent {
            authority: wire::IndependentAuthorityV1::ExternalBinding { .. },
            ..
        }
    ));
    fixture.freeze().await;
    coordinator
        .register_execution(&authority, &fixture.execution("binding-before-rotation"))
        .await
        .unwrap();
    let next = binding
        .rotate_credential(
            fixture::digest('b').as_str().into(),
            "2026-01-01T00:00:01Z".into(),
        )
        .unwrap();
    fixture
        .sandbox
        .save_binding(Some(binding.revision), next.clone())
        .await
        .unwrap();
    let execution = fixture.execution("binding-after-rotation");
    assert!(matches!(
        coordinator.register_execution(&authority, &execution).await,
        Err(ExecutionError::AuthorityChanged)
    ));
    assert!(matches!(
        fixture
            .store
            .register(
                &authority,
                &wire::MemberKey::Execution {
                    execution: execution.clone()
                }
            )
            .await,
        Err(ExecutionError::AuthorityChanged)
    ));
    assert!(coordinator
        .authorize_execution(
            &context(&principal),
            None,
            &fixture.target,
            ResourceCapability::Interact,
            Some(authority.basis())
        )
        .await
        .is_err());
    let fresh = bindings
        .authenticate(&next.key_sha256)
        .await
        .unwrap()
        .unwrap();
    let renewed = coordinator
        .authorize_execution(
            &context(&fresh),
            None,
            &fixture.target,
            ResourceCapability::Interact,
            Some(authority.basis()),
        )
        .await
        .unwrap();
    coordinator
        .register_execution(&renewed, &execution)
        .await
        .unwrap();
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn status_and_replay_recheck_expiry_while_worker_keeps_exact_finalization_authority() {
    use fixture::{context, Fixture};
    use layer2_core::resource_access::CredentialSource;
    use std::time::{Duration, SystemTime};
    let fixture = Fixture::new().await;
    let operation = fixture.freeze().await;
    for replay in [false, true] {
        let mut caller = context("owner");
        fixture
            .client
            .database("admin")
            .run_command(doc! {
                "configureFailPoint":"failCommand", "mode":{"times":1_i32},
                "data":{"failCommands":["find"], "appName":&fixture.app_name,
                    "blockConnection":true, "blockTimeMS":500_i32}
            })
            .await
            .unwrap();
        caller.credential_source = CredentialSource::Independent {
            expires_at: Some(SystemTime::now() + Duration::from_millis(200)),
        };
        let started = std::time::Instant::now();
        let result = if replay {
            fixture
                .coordinator
                .begin_revocation(&caller, None, fixture.command("freeze"))
                .await
        } else {
            fixture
                .coordinator
                .revocation_status(
                    &caller,
                    None,
                    &fixture.selector,
                    &operation.key.operation_id,
                )
                .await
        };
        fixture
            .client
            .database("admin")
            .run_command(doc! {
                "configureFailPoint":"failCommand", "mode":"off"
            })
            .await
            .unwrap();
        assert!(started.elapsed() >= Duration::from_millis(500));
        assert!(matches!(result, Err(ExecutionError::AuthorityChanged)));
    }
    let effective = fixture
        .coordinator
        .finish_revocation(&operation.key.operation_id)
        .await
        .unwrap();
    assert!(matches!(
        effective.phase,
        RevocationPhaseV1::Effective { .. }
    ));
    assert_eq!(effective.deadline_unix_ms, operation.deadline_unix_ms);
    let status = fixture
        .coordinator
        .revocation_status(
            &context("owner"),
            None,
            &fixture.selector,
            &operation.key.operation_id,
        )
        .await
        .unwrap();
    assert!(status == effective);
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn full_member_capacity_keeps_paged_revoke_evidence_available() {
    let fixture = fixture::Fixture::new().await;
    for index in 0..MAX_FAMILY_MEMBERS {
        fixture
            .register("recipient", &format!("member-{index}"))
            .await;
    }
    let authority = fixture.authority("recipient").await;
    assert!(matches!(
        fixture
            .coordinator
            .register_execution(&authority, &fixture.execution("overflow"))
            .await,
        Err(ExecutionError::Capacity)
    ));
    let operation = fixture.freeze().await;
    assert_eq!(operation.frozen_members, MAX_FAMILY_MEMBERS);
    let first = fixture
        .store
        .frozen_members(&operation.key.operation_id, 0, MAX_PAGE_MEMBERS)
        .await
        .unwrap();
    assert_eq!(first.len(), MAX_PAGE_MEMBERS as usize);
    let last_ordinal = first.last().unwrap().ordinal;
    for member in first {
        fixture.complete(&operation, &member).await;
    }
    assert!(fixture
        .store
        .finish_revocation(&operation.key.operation_id)
        .await
        .is_err());
    let second = fixture
        .store
        .frozen_members(&operation.key.operation_id, last_ordinal, MAX_PAGE_MEMBERS)
        .await
        .unwrap();
    assert_eq!(second.len(), MAX_PAGE_MEMBERS as usize);
    for member in second {
        fixture.complete(&operation, &member).await;
    }
    assert!(fixture
        .store
        .frozen_members(
            &operation.key.operation_id,
            MAX_FAMILY_MEMBERS,
            MAX_PAGE_MEMBERS
        )
        .await
        .unwrap()
        .is_empty());
    let final_operation = fixture
        .store
        .finish_revocation(&operation.key.operation_id)
        .await
        .unwrap();
    assert!(matches!(
        final_operation.phase,
        RevocationPhaseV1::Effective { .. }
    ));
    fixture.cleanup().await;
}
