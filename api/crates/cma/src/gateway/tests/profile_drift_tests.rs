use crate::agent_lifecycle::guidance::{
    compose_managed, managed_v2, managed_v3, ManagedInstructionPolicy,
};
use crate::auth::test_resources;
use layer2_core::profiles::{content_digest, AgentProvider};
use layer2_core::sandbox::{
    binding::{DriftState, ProvidedCredential},
    store::{AgentStore, SandboxStore, WorkspaceStore, WorktreeStore},
    AgentStatus, InMemorySandboxStore,
};
use layer2_core::{Authorization, Caller, Capability, SubjectId};
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn borrowed_binding_compares_owner_prompt_across_managed_guidance_revisions() {
    for (admitted_guidance, current_guidance) in [
        (managed_v3(), managed_v3()),
        (managed_v2(), managed_v2()),
        (managed_v2(), managed_v3()),
    ] {
        for (owner_prompt, expected) in [
            ("Keep the original instructions", DriftState::Match),
            ("Use the revised instructions", DriftState::Drift),
        ] {
            let store = Arc::new(InMemorySandboxStore::default());
            let owner = "fixture-owner";
            let mut agent = test_resources::agent(
                "agent",
                "worktree",
                "workspace",
                "sandbox",
                owner,
                AgentStatus::Ready,
            );
            let original =
                compose_managed(Some(admitted_guidance), "Keep the original instructions");
            agent.profile.content.system_prompt = original.clone();
            agent.ready_at = Some(agent.created_at.clone());
            agent.conversation.conversation_id = Some("fixture-conversation".into());
            agent.conversation.runtime_generation = Some(1);
            agent.conversation.managed_guidance_revision =
                u64::from(current_guidance == managed_v3());
            let mut settings = agent.settings();
            settings.system_prompt = Some(compose_managed(Some(current_guidance), owner_prompt));
            agent.conversation.settings = Some(settings);
            store
                .create_sandbox(test_resources::sandbox("sandbox", owner))
                .await
                .unwrap();
            store
                .create_workspace(test_resources::workspace("workspace", "sandbox", owner))
                .await
                .unwrap();
            store
                .create_worktree(test_resources::worktree(
                    "worktree",
                    "workspace",
                    "sandbox",
                    "agent",
                    owner,
                ))
                .await
                .unwrap();
            let agent_id = agent.id.clone();
            store.create_agent(agent).await.unwrap();

            let service = super::provider_fixture::service_for_store(store);
            let caller = Caller::new(
                SubjectId::new("fixture-manager"),
                Authorization::new([
                    Capability::ManageGatewayRecords,
                    Capability::ManageProviderBindings,
                ]),
            );
            let profile = json!({
                "schema_version":1,
                "metadata":{"name":"Borrowed Agent","owner":{"kind":"human","subject":owner},"provider":"registered"},
                "model":{"access":"managed","service":AgentProvider::default().connection_services().managed,
                    "api_type":"responses","model":"model-a","thinking_effort":"medium"},
                "permission":{"action_mode":"guarded"},"context":{},
                "environment":{"size":{"cpu_cores":1,"memory_gib":2},
                    "workspace":{"type":"directory","name":"project"}}
            });
            let digest = format!("sha256:{}", content_digest(&profile).unwrap());
            let key =
                ProvidedCredential::new(format!("nyx_{}", uuid::Uuid::new_v4().simple())).unwrap();
            let binding = service
                .put_existing_agent(&caller, "borrowed-agent", profile, &digest, &key, agent_id)
                .await
                .unwrap();
            assert_eq!(binding.profile.agent.system_prompt, original);
            service
                .ensure_conversation(&caller, "borrowed-agent", "fixture-partition")
                .await
                .unwrap();
            let inspection = service
                .inspect_conversation_with_profile_drift(
                    &caller,
                    "borrowed-agent",
                    "fixture-partition",
                    Some(&ManagedInstructionPolicy),
                )
                .await
                .unwrap();
            assert_eq!(inspection.profile_drift.unwrap().system_prompt, expected);
        }
    }
}
