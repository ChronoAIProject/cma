use super::*;
use layer2_core::{resource_access::ResourceOwner, SubjectId};

async fn setup(f: &Fixture, grant: &str) -> BotSetup {
    let mut setup: BotSetup = serde_json::from_value(json!({
        "id":"bst_connection", "actor":f.actor, "revision":1, "step":"external_bot",
        "grant_id":grant, "owner":{"kind":"person","subject":"visitor"},
        "draft":{"name":"Example","instructions":null},
        "configuration":{
            "agent":{"model":"fixture","thinking_effort":"medium","permission_level":"guarded","skills":[],"system_prompt":""},
            "workspace":{"location":{"type":"directory","name":"work"},"name":"work","setup_commands":[],"agent_profiles":[]},
            "sandbox":{"cpu_millis":1000,"memory_mib":2048,"nyxid_services":[],"agent_provider":"codex","model_access":"chronoai_managed","max_workspaces":1,"max_agents_per_workspace":1,"workspace_profiles":[],"software_install_commands":[],"environment_variables":[]},
            "availability":{"warmup_enabled":false,"warmup_count":1,"always_keep_alive":false}
        },
        "selected_services":[], "external_bot_id":null, "admission":null, "service_page":0,
        "confirmation_digest":null, "bot_id":null, "processed_events":[],
        "updated_at":layer2_core::now_rfc3339()
    })).unwrap();
    setup.pending_connection = Some(SetupConnectionRequest {
        record_id: uuid::Uuid::new_v4().to_string(),
        source_event_id: f.source.event_id.clone(),
        source_event_ref: f.source.reference.clone(),
        authority_stamp: f.authority.connection_stamp(&setup).await.unwrap(),
        plan_digest: setup.plan_digest().unwrap(),
        connection_ref: None,
        status: "pending".into(),
        setup_url: None,
    });
    setup
}

#[tokio::test]
async fn channel_connection_credentials_fence_exact_actor_owner_origin_and_incarnation() {
    let f = Fixture::new().await;
    let linked = f.activate().await;
    let setup = setup(&f, &linked.grant.grant_id).await;
    let (user, agent) = f.authority.connection_credentials(&setup).await.unwrap();
    assert_eq!(user.expose(), USER_TOKEN);
    assert_eq!(agent.expose(), KEY);
    // The Father binding belongs to a different owner; only the linked sender's
    // NyxID authority may create that sender's connection and CMA resources.
    assert_ne!(f.binding.owner.as_str(), "visitor");
    for field in [
        "actor",
        "channel",
        "conversation",
        "platform",
        "binding",
        "grant",
        "owner",
        "event",
        "reference",
        "incarnation",
    ] {
        let mut wrong = setup.clone();
        match field {
            "actor" => wrong.actor.actor_id = "bob".into(),
            "channel" => wrong.actor.channel_id = "other-channel".into(),
            "conversation" => wrong.actor.conversation_id = "other-chat".into(),
            "platform" => wrong.actor.platform = "other-platform".into(),
            "binding" => wrong.actor.binding_id = "other-binding".into(),
            "grant" => wrong.grant_id = Some("another-grant".into()),
            "owner" => {
                wrong.owner = Some(ResourceOwner::Person {
                    subject: SubjectId::new("bob"),
                })
            }
            "event" => {
                wrong.pending_connection.as_mut().unwrap().source_event_id = "other-event".into()
            }
            "reference" => {
                wrong.pending_connection.as_mut().unwrap().source_event_ref =
                    "cmaeg1.ev.other".into()
            }
            _ => {
                wrong.pending_connection.as_mut().unwrap().authority_stamp =
                    "wrong-incarnation".into()
            }
        }
        assert!(
            matches!(
                f.authority.connection_credentials(&wrong).await,
                Err(BotError::Forbidden)
            ),
            "{field}"
        );
    }
    let mut organization = setup.clone();
    organization.owner = Some(ResourceOwner::Organization {
        subject: SubjectId::new("visitor-org"),
    });
    f.authority
        .connection_credentials(&organization)
        .await
        .unwrap();
    organization.owner = Some(ResourceOwner::Organization {
        subject: SubjectId::new("someone-elses-org"),
    });
    assert!(matches!(
        f.authority.connection_credentials(&organization).await,
        Err(BotError::Forbidden)
    ));
    f.authority
        .auth
        .revoke_actor_link(&linked.actor, &linked.grant.grant_id)
        .await
        .unwrap();
    assert!(matches!(
        f.authority.connection_credentials(&setup).await,
        Err(BotError::Forbidden)
    ));
}
