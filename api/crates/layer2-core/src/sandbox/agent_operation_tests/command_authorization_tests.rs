use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use layer3_adapter::ConversationInputSender;

use super::command_work_tests::{admit, fixture, request};
use super::fixtures::*;
use crate::sandbox::agent_io::*;
use crate::sandbox::store::{AgentLogStore, AgentStore};
use crate::sandbox::*;
use crate::Caller;

struct Authorizer {
    answer: Result<Caller, CommandAuthorizationError>,
    calls: AtomicUsize,
}
#[async_trait::async_trait]
impl AgentCommandAuthorizer for Authorizer {
    async fn authorize_command(
        &self,
        envelope: &AgentCommandEnvelope,
    ) -> Result<Caller, CommandAuthorizationError> {
        assert_eq!(envelope.agent_id.as_str(), "agt_operations");
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.answer.clone()
    }
}

#[tokio::test]
async fn no_runtime_or_ledger_effect_precedes_current_authorization() {
    let (store, agent, parent) = fixture(AgentStatus::Ready).await;
    admit(
        &store,
        &agent,
        &parent,
        "authorize",
        AgentCommand::message("Inspect this."),
    )
    .await;
    let claimed = store
        .claim_next_command(request(&agent, "worker", &crate::now_rfc3339()))
        .await
        .unwrap()
        .unwrap();
    let sends = Arc::new(AtomicUsize::new(0));
    let sent = sends.clone();
    let sender = ConversationInputSender::spawn(4, move |_| {
        sent.fetch_add(1, Ordering::SeqCst);
        async { Ok(()) }
    });
    let service = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    for answer in [
        Err(CommandAuthorizationError::Revoked),
        Err(CommandAuthorizationError::Unavailable),
        Ok(other_owner()),
        Ok(unauthorized_owner()),
    ] {
        let authorizer = Authorizer {
            answer,
            calls: AtomicUsize::new(0),
        };
        assert!(service
            .execute_authorized_claim(&authorizer, &claimed)
            .await
            .is_err());
        assert_eq!(authorizer.calls.load(Ordering::SeqCst), 1);
        assert_eq!(sends.load(Ordering::SeqCst), 0);
        assert_eq!(
            store.get_agent(&agent.id).await.unwrap(),
            Some(agent.clone())
        );
        assert_eq!(
            store
                .get_command(&claimed.receipt.envelope.command_id)
                .await
                .unwrap(),
            Some(claimed.clone())
        );
    }
    let authorizer = Authorizer {
        answer: Ok(owner()),
        calls: AtomicUsize::new(0),
    };
    service
        .execute_authorized_claim(&authorizer, &claimed)
        .await
        .unwrap();
    let stored = store
        .get_command(&claimed.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    assert!(stored.work.dispatch.is_some());
    assert!(service
        .execute_authorized_claim(&authorizer, &stored)
        .await
        .is_err());
    assert_eq!(authorizer.calls.load(Ordering::SeqCst), 1);
    assert!(service
        .execute_authorized_claim(&authorizer, &claimed)
        .await
        .is_err());
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    assert_eq!(
        store
            .agent_log(&agent.id, 0, 50)
            .await
            .unwrap()
            .iter()
            .filter(|entry| matches!(entry.kind, AgentLogKind::OwnerMessage { .. }))
            .count(),
        1
    );
}

struct ReleasingAuthorizer {
    store: Arc<InMemorySandboxStore>,
    claim: CommandClaim,
}
#[async_trait::async_trait]
impl AgentCommandAuthorizer for ReleasingAuthorizer {
    async fn authorize_command(
        &self,
        _: &AgentCommandEnvelope,
    ) -> Result<Caller, CommandAuthorizationError> {
        self.store
            .release_command_claim(&self.claim, crate::now_rfc3339())
            .await
            .unwrap();
        Ok(owner())
    }
}

#[tokio::test]
async fn lease_loss_during_provider_io_is_fenced_before_runtime_input() {
    let (store, agent, parent) = fixture(AgentStatus::Ready).await;
    admit(
        &store,
        &agent,
        &parent,
        "lease-loss",
        AgentCommand::message("Inspect this."),
    )
    .await;
    let claimed = store
        .claim_next_command(request(&agent, "worker", &crate::now_rfc3339()))
        .await
        .unwrap()
        .unwrap();
    let sends = Arc::new(AtomicUsize::new(0));
    let sent = sends.clone();
    let sender = ConversationInputSender::spawn(4, move |_| {
        sent.fetch_add(1, Ordering::SeqCst);
        async { Ok(()) }
    });
    let service = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    let authorizer = ReleasingAuthorizer {
        store: store.clone(),
        claim: claimed.work.lease.as_ref().unwrap().claim.clone(),
    };
    assert!(service
        .execute_authorized_claim(&authorizer, &claimed)
        .await
        .is_err());
    assert_eq!(sends.load(Ordering::SeqCst), 0);
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
    assert!(store
        .get_command(&claimed.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap()
        .work
        .dispatch
        .is_none());
}
