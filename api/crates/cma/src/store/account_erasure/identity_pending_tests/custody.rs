use super::*;
use layer2_core::bots::{Bot, BotOwnership, BotStore};
use layer2_core::profiles::content_digest;
use layer2_core::sandbox::channel_usage::*;
use mongodb::bson::{self, bson};
use serde_json::json;

fn bot() -> Bot {
    let now = NOW;
    let agent = json!({"model":"test","thinking_effort":"medium","permission_level":"guarded","skills":[],"system_prompt":""});
    let sandbox = json!({"cpu_millis":1000,"memory_mib":2048,"nyxid_services":[],"agent_provider":"codex","model_access":"chronoai_managed","max_workspaces":1,"max_agents_per_workspace":1,"workspace_profiles":[],"software_install_commands":[],"environment_variables":[]});
    let workspace = json!({"location":{"type":"directory","name":"project"},"name":"project","setup_commands":[],"agent_profiles":[]});
    let mut bot: Bot = serde_json::from_value(json!({
        "bot_id":"bot_organization", "owner":"organization",
        "request":{"name":"test","external_bot_id":"external-test-bot","profile":{"id":"agp_test","revision":1},"admission":{"type":"all"},"reply":{"auto_final":true,"ack_after_ms":2000}},
        "profiles":{
            "resolved_model":"test",
            "agent":{"profile_id":"agp_test","revision":1,"digest":content_digest(&agent).unwrap(),"content":agent,"published_at":now},
            "sandbox":{"profile_id":"sbp_test","revision":1,"digest":content_digest(&sandbox).unwrap(),"content":sandbox,"published_at":now},
            "workspace":{"profile_id":"wkp_test","revision":1,"digest":content_digest(&workspace).unwrap(),"content":workspace,"published_at":now}
        },
        "provider":"registered","record_id":uuid::Uuid::new_v4().to_string(),
        "resources":{"channel":null,"route_id":null,"credentials":{},"replacement":null},
        "state":"creating","work":{"id":"test-work-id","operation":"create","step":"admitted","generation":1,"holder":null,"expires_at":null},
        "generation":1,"revision":1,"creation_key_hash":"a".repeat(64),
        "request_fingerprint":"b".repeat(64),"created_at":now,"updated_at":now,"last_test":null
    })).unwrap();
    bot.ownership = Some(BotOwnership {
        owner: ResourceOwner::Organization {
            subject: SubjectId::new("organization"),
        },
        created_by: ResourceActor::Api {
            subject: SubjectId::new("a"),
            delegation_id: Some("organization-delegation".into()),
        },
        credential_subject: SubjectId::new("a"),
        credential_incarnation: Some("organization-sign-in".into()),
        authorization_grant_id: Some("organization-delegation".into()),
    });
    bot.validate().unwrap();
    bot
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn organization_bot_keeps_credential_subject_and_exact_delegation_tuple() {
    let db = database().await;
    let store = crate::store::bots::MongoBotStore::open(db.client(), db.name())
        .await
        .unwrap();
    let bot = bot();
    store.insert(&bot).await.unwrap();
    let original = row(&db, "bots", &bson!(bot.bot_id.clone())).await;
    preserved(&db, "bots", &original).await;
    assert_eq!(store.get(&bot.bot_id).await.unwrap(), Some(bot));
    db.drop().await.unwrap();
}

fn channel_rows(creation_subject: &str, event_subject: &str) -> (Document, Document) {
    let creation = provenance(creation_subject);
    let before = AgentChannelUsageState::empty(creation.clone());
    let binding = ChannelBindingReference {
        binding_id: "historical-binding".into(),
        bridge_id: "historical-bridge".into(),
        partition_id: "historical-partition".into(),
        generation: 1,
        previous_binding_id: None,
    };
    let change = ChannelUsageChange::Attach {
        binding: binding.clone(),
    };
    let mut after = before.clone();
    after.revision = 1;
    after.last_changed_at = Some(NOW.into());
    after.active_bindings.push(ActiveChannelBinding {
        reference: binding,
        attached_at: NOW.into(),
        first_used_at: None,
        last_used_at: None,
    });
    let input = ChannelUsageMutation {
        creation,
        policy_fence: layer2_core::resource_access::ResourcePolicyFence {
            sandbox_id: SandboxId("sbx_foreign".into()),
            sandbox_created_at: NOW.into(),
            policy_revision: 1,
        },
        expected_revision: 0,
        key_sha256: "d".repeat(64),
        actor: actor(event_subject),
        change: change.clone(),
        reason: ChannelBindingChangeReason::Attachment,
        at: NOW.into(),
    };
    input.validate().unwrap();
    // The typed event validates the exact before/after transition and digest.
    // Serialize its ordinary writer shape without depending on C3 codec exports.
    let event: ChannelUsageEvent = serde_json::from_value(json!({
        "operation_id":input.operation_id(), "fingerprint":input.fingerprint(),
        "actor":input.actor, "at":NOW, "detached_at":null,
        "reason":input.reason, "change":change, "predecessor":null,
        "before":before, "after":after
    }))
    .unwrap();
    event.validate().unwrap();
    let event_row = doc! {
        "_id":&event.operation_id, "document_schema_version":1_i64,
        "agent_id":"agt_foreign", "revision":1_i64,
        "binding_id":"historical-binding", "digest":event.digest(),
        "record":bson::serialize_to_bson(&event).unwrap()
    };
    let stored = StoredAgentChannelUsage {
        state: event.after.clone(),
        latest_event_digest: event.digest(),
    };
    stored.validate_latest(&event).unwrap();
    let state_row = doc! {
        "_id":"agt_foreign", "document_schema_version":1_i64,
        "revision":1_i64, "active":true,
        "record":bson::serialize_to_bson(&stored).unwrap()
    };
    (state_row, event_row)
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn foreign_channel_snapshot_creation_and_event_actors_remain_pending_and_readable() {
    let (state, event_with_creation) = channel_rows("a", "survivor");
    let (_, event_with_actor) = channel_rows("survivor", "a");
    for (name, original) in [
        ("agent_channel_usage", state),
        ("channel_usage_events", event_with_creation),
        ("channel_usage_events", event_with_actor),
    ] {
        let db = database().await;
        put(&db, name, original.clone()).await;
        preserved(&db, name, &original).await;
        if name == "agent_channel_usage" {
            let stored: StoredAgentChannelUsage = crate::store::sandbox_mongo::read_record(
                &row(&db, name, original.get("_id").unwrap()).await,
            )
            .unwrap();
            stored.state.validate().unwrap();
        } else {
            let event: ChannelUsageEvent = crate::store::sandbox_mongo::read_record(
                &row(&db, name, original.get("_id").unwrap()).await,
            )
            .unwrap();
            event.validate().unwrap();
            assert_eq!(original.get_str("digest").unwrap(), event.digest());
        }
        db.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn unrelated_bot_and_channel_attribution_allow_personal_erasure() {
    let db = database().await;
    sandbox(&db, "a", "sbx_personal").await;
    let store = crate::store::bots::MongoBotStore::open(db.client(), db.name())
        .await
        .unwrap();
    let mut bot = bot();
    let ownership = bot.ownership.as_mut().unwrap();
    ownership.created_by = ResourceActor::Api {
        subject: SubjectId::new("survivor"),
        delegation_id: ownership.authorization_grant_id.clone(),
    };
    ownership.credential_subject = SubjectId::new("survivor");
    bot.validate().unwrap();
    store.insert(&bot).await.unwrap();
    let bot_row = row(&db, "bots", &bson!(bot.bot_id.clone())).await;
    let (state, event) = channel_rows("survivor", "survivor");
    let originals = [
        ("bots", bot_row),
        ("agent_channel_usage", state),
        ("channel_usage_events", event),
    ];
    for (name, original) in &originals {
        if *name != "bots" {
            put(&db, name, original.clone()).await;
        }
        assert_eq!(
            super::super::references::redact(&db, name, original, &BTreeSet::from(["a".into()]),)
                .await
                .unwrap(),
            Redaction::Unchanged
        );
    }
    drain(&db, "a", &mut ErasureInventory::default()).await;
    assert_eq!(count(&db, "sandboxes").await, 0);
    for (name, original) in originals {
        assert_eq!(row(&db, name, original.get("_id").unwrap()).await, original);
    }
    assert_eq!(store.get(&bot.bot_id).await.unwrap(), Some(bot));
    db.drop().await.unwrap();
}
