use super::{
    command_tests::{request, LocalAuthority},
    resource_access::{context, ready_conversation},
    *,
};
use crate::{resource_access::ResourceAccessService, sandbox::agent_io::*};
use std::sync::atomic::{AtomicBool, Ordering};

struct Source(AtomicBool);
struct PendingSource;
#[async_trait::async_trait]
impl CommandSourceGuard for PendingSource {
    async fn check(&self, _: &AgentCommandEnvelope) -> Result<(), CommandAuthorizationError> {
        std::future::pending().await
    }
}
#[async_trait::async_trait]
impl CommandSourceGuard for Source {
    async fn check(&self, _: &AgentCommandEnvelope) -> Result<(), CommandAuthorizationError> {
        if self.0.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(CommandAuthorizationError::Revoked)
        }
    }
}
#[tokio::test]
async fn continuation_is_exact_parent_append_replay_and_fresh_source_authority() {
    let f = fixture();
    let binding = f.ready().await;
    ready_conversation(&f, &binding).await;
    let bindings = BindingAccessService::new(f.store.clone());
    let authority = Arc::new(LocalAuthority(bindings.clone()));
    let access = Arc::new(
        ResourceAccessService::new(f.store.clone(), authority.clone())
            .with_bindings(bindings.clone()),
    );
    let commands = AgentCommandService::new(f.store.clone(), access.clone(), authority);
    let mut request = request(&f, &bindings, &access, "original-event").await;
    super::event_tests::attach(&mut request);
    let CommandReservationResult::Created(parent) = f.store.reserve_command(request).await.unwrap()
    else {
        panic!()
    };
    let principal = bindings
        .authenticate(&f.key.sha256())
        .await
        .unwrap()
        .unwrap();
    let context = context(&principal);
    let source = Arc::new(Source(AtomicBool::new(true)));
    let proof = ExternalEventContinuation {
        parent_command_id: parent.receipt.envelope.command_id.clone(),
        parent_response_id: parent.receipt.response_id.clone().unwrap(),
        parent_fingerprint: parent.receipt.envelope.request_fingerprint.clone(),
        binding_generation: parent
            .receipt
            .envelope
            .authorization
            .grant_revision
            .unwrap(),
        source_generation: None,
        source_expires_at: (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 172_800) as i64,
        artifact_id: "0:transcript".into(),
        source_proof: json!({"exact":"source-owned-proof"}),
    };
    let accepted = commands
        .admit_external_continuation(
            &context,
            "",
            proof.clone(),
            "late transcript".into(),
            &*source,
        )
        .await
        .unwrap();
    let saved = f
        .store
        .get_command(&accepted.command.receipt.command_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        saved.receipt.envelope.agent_id,
        parent.receipt.envelope.agent_id
    );
    assert!(
        matches!(&saved.receipt.envelope.command, AgentCommand::Message { order: AgentInputOrder::Append, input, metadata } if input.external_event().is_none() && metadata.is_empty())
    );
    assert!(
        crate::seconds_from_rfc3339(&saved.queue_deadline).unwrap()
            <= crate::seconds_from_rfc3339(&saved.receipt.admitted_at).unwrap() + 86_400
    );
    assert!(
        crate::seconds_from_rfc3339(&saved.queue_deadline).unwrap()
            > crate::seconds_from_rfc3339(&parent.queue_deadline).unwrap()
    );
    assert_eq!(
        accepted.metadata["cma_parent_response_id"],
        proof.parent_response_id.to_string()
    );
    assert_eq!(accepted.metadata["cma_artifact_id"], "0:transcript");
    assert_eq!(
        accepted.metadata,
        saved.receipt.envelope.response_metadata()
    );
    let replay = commands
        .admit_external_continuation(
            &context,
            "",
            proof.clone(),
            "late transcript".into(),
            &*source,
        )
        .await
        .unwrap();
    assert_eq!(accepted.command, replay.command);
    assert_eq!(accepted.metadata, replay.metadata);
    assert!(matches!(
        commands
            .admit_external_continuation(
                &context,
                "",
                proof.clone(),
                "changed transcript".into(),
                &*source
            )
            .await,
        Err(CommandAdmissionError::Ledger(
            CommandLedgerError::IdempotencyConflict
        ))
    ));
    let mut foreign = proof.clone();
    foreign.parent_response_id = crate::ids::AgentResponseId::generate();
    assert!(commands
        .admit_external_continuation(&context, "", foreign, "late transcript".into(), &*source)
        .await
        .is_err());
    let execution = access
        .authorize_command_execution(&context, "", &saved.receipt.envelope)
        .await
        .unwrap();
    assert!(execution.current_snapshot().await.is_err());
    let execution = execution.with_source_guard(source.clone());
    execution.current_snapshot().await.unwrap();
    source.0.store(false, Ordering::SeqCst);
    assert!(execution.current_snapshot().await.is_err());
    assert!(commands
        .admit_external_continuation(&context, "", proof, "late transcript".into(), &*source)
        .await
        .is_err());
    assert_eq!(
        f.store
            .get_command(&parent.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap()
            .receipt,
        parent.receipt
    );
    let mut invalid = saved.clone();
    invalid.queue_deadline = crate::rfc3339_at(
        std::time::UNIX_EPOCH
            + std::time::Duration::from_secs(
                crate::seconds_from_rfc3339(&saved.receipt.admitted_at).unwrap() + 86_401,
            ),
    );
    assert_eq!(invalid.validate(), Err(CommandLedgerError::Corrupt));
    // An attestation exceeding the live proof deadline is an outage, never a
    // durable revocation or permission to write without a source answer.
    tokio::time::pause();
    let timed_out = execution.with_source_guard(Arc::new(PendingSource));
    assert!(matches!(
        timed_out.current_snapshot().await,
        Err(crate::resource_access::ResourceAccessError::Store(
            crate::resource_access::ResourceAccessStoreError::Unavailable
        ))
    ));
    tokio::time::resume();
}
