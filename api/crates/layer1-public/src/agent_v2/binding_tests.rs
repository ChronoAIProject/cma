use super::{fixtures::*, AgentV2State};
use async_trait::async_trait;
use axum::http::{Method, StatusCode};
use layer2_core::{
    ids::{AgentCommandId, AgentResponseId},
    sandbox::agent_io::*,
    AgentId, SubjectId,
};
use std::sync::Arc;

struct AlteredCommands {
    inner: Arc<dyn AgentCommandStore>,
    fault: &'static str,
}
#[async_trait]
impl AgentCommandStore for AlteredCommands {
    async fn reserve_command(
        &self,
        _: CommandReservation,
    ) -> Result<CommandReservationResult, CommandLedgerError> {
        panic!("replay must never admit a command")
    }
    async fn get_response_command(
        &self,
        id: &AgentResponseId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        self.inner.get_response_command(id).await
    }
    async fn get_command(
        &self,
        id: &AgentCommandId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        if self.fault == "unavailable" {
            return Err(CommandLedgerError::Unavailable);
        }
        let Some(mut record) = self.inner.get_command(id).await? else {
            return Ok(None);
        };
        let envelope = &mut record.receipt.envelope;
        let dispatch = record.work.dispatch.as_mut().unwrap();
        match self.fault {
            "command" => {
                envelope.command_id = AgentCommandId("cmd_foreign".into());
                dispatch.claim.command_id = envelope.command_id.clone();
            }
            "agent" => {
                envelope.agent_id = AgentId("agt_foreign".into());
                dispatch.claim.agent_id = envelope.agent_id.clone();
            }
            "owner" => {
                envelope.owner = AgentOwner::Person {
                    subject: SubjectId::new("foreign-owner"),
                }
            }
            "turn" => dispatch.turn_sequence += 1,
            "fence" => dispatch.fenced_at = "2026-09-05T00:00:00Z".into(),
            "response" => record.receipt.response_id = None,
            "incarnation" => record.receipt.admitted_at = "2020-09-05T00:00:00Z".into(),
            "control" => {
                envelope.command = AgentCommand::Interrupt;
                envelope.request_fingerprint = envelope.command.fingerprint().unwrap();
            }
            _ => unreachable!(),
        }
        Ok(Some(record))
    }
}

#[tokio::test]
async fn replay_refuses_unverifiable_source_identity_turn_owner_incarnation_and_receipt() {
    for enabled in [false, true] {
        for fault in [
            "command",
            "agent",
            "owner",
            "turn",
            "fence",
            "response",
            "incarnation",
            "control",
            "unavailable",
        ] {
            let f = fixture(true, false).await;
            let commands = AgentCommandService::new(
                Arc::new(AlteredCommands {
                    inner: f.store.clone(),
                    fault,
                }),
                f.access.clone(),
                Arc::new(NoAdmission),
            );
            let state =
                AgentV2State::new(f.state.reads.clone(), commands).with_single_fence_pages(enabled);
            let router = super::router(state);
            let (status, body) = call(&router, Method::GET, SNAPSHOT, "owner-a", None, None).await;
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{fault}: {body}");
            assert_eq!(
                body["code"],
                if fault == "unavailable" {
                    "agent_read_unavailable"
                } else {
                    "agent_history_unavailable"
                }
            );
            for secret in ["foreign-owner", "private-source", "runtime", "cmd_foreign"] {
                assert!(
                    !body.to_string().contains(secret),
                    "{fault} leaked {secret}"
                );
            }
        }
    }
}
