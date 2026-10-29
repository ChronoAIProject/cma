use super::Scenario;
use crate::responses::v2::tests::{context, fixture};
use axum::Extension;
use layer2_core::{
    ids::{AgentCommandId, AgentResponseId},
    resource_access::ResourcePolicyFence,
    sandbox::{
        agent_io::*,
        store::{AgentStore, SandboxMutationGuard, SandboxStore},
    },
    AgentId,
};

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    let mode = name.strip_prefix("responses_v2_")?;
    assert!(["owner", "foreign", "anonymous", "failed"].contains(&mode));
    let f = fixture().await;
    let agent = f
        .store
        .get_agent(&AgentId("agt_fixture".into()))
        .await
        .unwrap()
        .unwrap();
    let parent = f
        .store
        .get_sandbox(&agent.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let command = AgentCommand::message("Hello");
    // A fixed retained receipt makes replay bytes comparable across SDKs. Fresh
    // admission and random receipt identity are exercised by actual-handler and
    // official SDK tests; these cases cover recovery of existing durable work.
    f.store
        .reserve_command(CommandReservation {
            binding: None,
            envelope: AgentCommandEnvelope {
                continuation: None,
                schema_version: AGENT_IO_SCHEMA_VERSION,
                command_id: AgentCommandId("cmd_contract".into()),
                agent_id: agent.id.clone(),
                owner: AgentOwner::Person {
                    subject: agent.owner.clone(),
                },
                actor: context("owner-a").actor,
                source: AgentCommandSource {
                    kind: AgentSourceKind::Api,
                    reference: "cmd_contract".into(),
                },
                correlation_id: "cmd_contract".into(),
                request_fingerprint: command.fingerprint().unwrap(),
                command,
                authorization: AgentAuthorizationRevision {
                    authority_id: "fixture-custody".into(),
                    policy_revision: 1,
                    grant_revision: None,
                },
            },
            key_sha256: command_key_digest("fixture-key").unwrap(),
            response_id: Some(AgentResponseId("resp_contract".into())),
            admitted_at: "2026-09-05T00:00:00Z".into(),
            queue_deadline: layer2_core::rfc3339_at(
                std::time::SystemTime::now() + std::time::Duration::from_secs(300),
            ),
            queue_capacity: 64,
            expected_agent_revision: agent.revision,
            parent: SandboxMutationGuard::capture(&parent),
            policy: ResourcePolicyFence {
                sandbox_id: parent.id,
                sandbox_created_at: parent.created_at,
                policy_revision: 0,
            },
            authority_deadline: None,
            admission_audit: None,
        })
        .await
        .unwrap();
    if mode == "failed" {
        f.store
            .reject_unclaimed_command(
                &AgentCommandId("cmd_contract".into()),
                1,
                CommandFailureCode::AuthorityRevoked,
                layer2_core::now_rfc3339(),
            )
            .await
            .unwrap();
    }
    let mut symbols = std::collections::BTreeMap::new();
    if mode != "failed" {
        let runtime = std::sync::Arc::new(super::agent_cases::ContractRuntime::completing());
        runtime.attach_store(f.store.clone());
        let operations = layer2_core::sandbox::AgentOperationService::new(f.store.clone(), runtime);
        let now = std::time::SystemTime::now();
        let claimed = f
            .store
            .claim_next_command(CommandClaimRequest {
                agent_id: agent.id.clone(),
                holder: "contract-worker".into(),
                now: layer2_core::rfc3339_at(now),
                expires_at: layer2_core::rfc3339_at(now + std::time::Duration::from_secs(30)),
            })
            .await
            .unwrap()
            .unwrap();
        operations
            .execute_claimed_command(&context("owner-a").caller, &claimed)
            .await
            .unwrap();
        f.store
            .reconcile_command(&claimed.receipt.envelope.command_id)
            .await
            .unwrap();
        if mode == "owner" {
            use layer2_core::sandbox::store::AgentLogStore;
            let history = f.store.agent_log(&agent.id, 0, 100).await.unwrap();
            let completed = history
                .iter()
                .find(|entry| {
                    matches!(
                        entry.kind,
                        layer2_core::sandbox::AgentLogKind::TurnCompleted { .. }
                    )
                })
                .unwrap();
            symbols.insert(
                "completed_at".into(),
                serde_json::json!(
                    layer2_core::seconds_from_rfc3339(&completed.appended_at).unwrap()
                ),
            );
        }
    }
    let subject = if mode == "foreign" {
        "foreign"
    } else {
        "owner-a"
    };
    let caller = context(subject);
    Some(Scenario {
        router: f.router.layer(Extension(caller.clone())).layer(Extension(
            crate::DelegatedCredential::new("fixture-credential".into()),
        )),
        subject: (mode != "anonymous").then_some(subject),
        authorization: caller.caller.authorization,
        symbols,
    })
}
