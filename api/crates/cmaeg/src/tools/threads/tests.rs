use super::*;
use axum::http::StatusCode;
use cmaeg_core::channel::{Secret, Source};
use fixture::{thread, App};
use serde_json::{json, Value};
use std::sync::atomic::Ordering;

async fn body(response: reqwest::Response) -> Value {
    let status = response.status();
    let value = response.json().await.unwrap();
    assert_eq!(status, 200, "{value}");
    value
}
#[tokio::test]
async fn list_filters_private_foreign_guild_parent_and_retains_expiry_consumer_and_actor() {
    for named in [false, true] {
        let app = App::new(None, named).await;
        let mut other_guild = thread("703", 11, "98", false);
        other_guild["guild_id"] = "200".into();
        app.memory.lock().unwrap().active = json!({"threads":[thread("701",11,"98",false),thread("702",12,"98",false),other_guild,thread("704",11,"97",false)],"members":[{"id":"secret-membership"}]});
        let value = body(app.list("active").await).await;
        assert_eq!(value["scope"], "public_children_of_original_parent");
        assert_eq!(value["threads"].as_array().unwrap().len(), 1);
        assert!(!value.to_string().contains("secret-membership"));
        let reference: ThreadRef =
            serde_json::from_value(value["threads"][0]["thread_ref"].clone()).unwrap();
        let payload = reference.open(&app.state.keys, now()).unwrap().payload;
        assert_eq!(payload.event, app.event);
        assert_eq!(payload.expires_at, app.event.expires_at);
        assert_eq!(payload.parent_id, "98");
        assert_eq!(payload.thread_type, 11);
        let channel = app
            .state
            .resolver
            .resolve_current(app.event.channel_id.as_str())
            .await
            .unwrap();
        let authority = thread_authority::load(
            app.state.resolver.record_client(),
            app.state.keys.clone(),
            &channel,
            &app.event,
        )
        .await
        .unwrap();
        assert_eq!(authority.activity.actor.id, "51");
        assert!(authority.target(&payload).is_ok());
        let mut private = payload.clone();
        private.thread_type = 12;
        assert!(authority.target(&private).is_err());
        let mut foreign = payload.clone();
        foreign.parent_id = "97".into();
        assert!(authority.target(&foreign).is_err());
        let mut unrelated = payload;
        unrelated.event.consumer = if named {
            None
        } else {
            Some(
                cmaeg_core::ids::ConsumerId::parse(crate::channels::routing_fixture::REVIEWER)
                    .unwrap(),
            )
        };
        assert!(authority.target(&unrelated).is_err());
        assert!(app.writes().is_empty());
    }
}
#[tokio::test]
async fn list_is_bounded_and_rejects_caller_cursor_and_destinations() {
    let app = App::new(None, false).await;
    app.memory.lock().unwrap().active = json!({"threads":(800..830).map(|id|thread(&id.to_string(),11,"98",false)).collect::<Vec<_>>()});
    let value = body(app.list("active").await).await;
    assert_eq!(value["threads"].as_array().unwrap().len(), 25);
    assert_eq!(value["truncated"], true);
    app.memory.lock().unwrap().archived["has_more"] = true.into();
    let value = body(app.list("archived").await).await;
    assert_eq!(value["threads"][0]["archived"], true);
    assert_eq!(value["truncated"], true);
    for view in [
        "active&before=2026-01-01",
        "active&parent_id=97",
        "active&guild_id=200",
        "unknown",
    ] {
        assert_eq!(app.list(view).await.status(), 400);
    }
}
#[tokio::test]
async fn existing_thread_never_widens_to_siblings_even_for_private_and_forum_parents() {
    for kind in [10, 11, 12] {
        let app = App::new(Some(kind), false).await;
        let value = body(app.list("active").await).await;
        assert_eq!(value["scope"], "original_thread");
        assert_eq!(value["threads"].as_array().unwrap().len(), 1);
        assert_eq!(value["threads"][0]["thread_id"], "99");
        assert!(!app
            .memory
            .lock()
            .unwrap()
            .calls
            .iter()
            .any(|(_, path, _)| path.contains("/guilds/") || path.contains("/archived/")));
        assert_eq!(
            body(app.create("create").await).await["outcome"]["status"],
            "unsupported"
        );
        assert!(app.writes().is_empty());
    }
}
#[tokio::test]
async fn anchored_create_returns_original_expiry_replays_and_conflicts_without_a_second_post() {
    for named in [false, true] {
        let app = App::new(None, named).await;
        let value = body(app.create("create-one").await).await;
        assert_eq!(value["outcome"]["status"], "confirmed");
        assert_eq!(value["thread"]["thread_id"], "700");
        assert_eq!(value["thread"]["expires_at"], app.event.expires_at);
        assert_eq!(body(app.create("create-one").await).await, value);
        let changed = reqwest::Client::new()
            .post(format!(
                "{}/v1/events/{}/threads",
                app.base,
                app.reference().as_str()
            ))
            .bearer_auth(&app.key)
            .header("Idempotency-Key", "create-one")
            .json(&json!({"name":"Different","auto_archive_duration":1440}))
            .send()
            .await
            .unwrap();
        assert_eq!(changed.status(), 409);
        assert_eq!(
            app.writes(),
            vec![(
                "POST".into(),
                "/api/v10/channels/98/messages/700/threads".into(),
                json!({"name":"Review","auto_archive_duration":1440})
            )]
        );
        let target = json!({"type":"thread","thread_ref":value["thread"]["thread_ref"]});
        assert_eq!(
            body(app.manage(target, "archive", "archive-created").await).await["outcome"]["status"],
            "confirmed"
        );
    }
}
#[tokio::test]
async fn archive_reopen_preserve_locks_and_membership_uses_only_empty_204_at_me() {
    let app = App::new(Some(11), false).await;
    app.memory.lock().unwrap().channels.get_mut("99").unwrap()["thread_metadata"]["locked"] =
        true.into();
    for action in ["archive", "reopen", "join", "leave"] {
        let value = body(app.manage(app.target(), action, action).await).await;
        assert_eq!(value["outcome"]["status"], "confirmed", "{value}");
    }
    let writes = app.writes();
    assert_eq!(writes.len(), 4);
    assert_eq!(
        writes[0],
        (
            "PATCH".into(),
            "/api/v10/channels/99".into(),
            json!({"archived":true})
        )
    );
    assert_eq!(
        writes[1],
        (
            "PATCH".into(),
            "/api/v10/channels/99".into(),
            json!({"archived":false})
        )
    );
    assert_eq!(
        writes[2],
        (
            "PUT".into(),
            "/api/v10/channels/99/thread-members/@me".into(),
            Value::Null
        )
    );
    assert_eq!(
        writes[3],
        (
            "DELETE".into(),
            "/api/v10/channels/99/thread-members/@me".into(),
            Value::Null
        )
    );
    assert_eq!(
        app.memory.lock().unwrap().channels["99"]["thread_metadata"]["locked"],
        true
    );
}
#[tokio::test]
async fn archived_membership_is_unsupported_and_platform_permission_refusal_is_explicit() {
    let app = App::new(Some(12), true).await;
    app.memory.lock().unwrap().channels.get_mut("99").unwrap()["thread_metadata"]["archived"] =
        true.into();
    for action in ["join", "leave"] {
        let value = body(app.manage(app.target(), action, action).await).await;
        assert_eq!(value["outcome"]["status"], "unsupported");
        assert_eq!(
            value["outcome"]["reason"],
            "membership_requires_active_thread"
        );
    }
    assert!(app.writes().is_empty());
    app.memory.lock().unwrap().status = StatusCode::FORBIDDEN;
    let value = body(app.manage(app.target(), "reopen", "reopen").await).await;
    assert_eq!(value["outcome"]["status"], "rejected");
    assert_eq!(
        value["outcome"]["reason"],
        "thread_permission_or_target_unavailable"
    );
}
#[tokio::test]
async fn uncertain_create_reconciles_only_exact_bot_owned_result_without_retrying_the_write() {
    for accepted in [false, true] {
        let app = App::new(None, false).await;
        {
            let mut memory = app.memory.lock().unwrap();
            memory.status = StatusCode::BAD_GATEWAY;
            memory.create_accepted = accepted;
        }
        let value = body(app.create("uncertain").await).await;
        assert_eq!(
            value["outcome"]["status"],
            if accepted { "confirmed" } else { "unknown" }
        );
        assert_eq!(body(app.create("uncertain").await).await, value);
        let next = body(app.create("different-key").await).await;
        assert_eq!(
            next["outcome"]["status"],
            if accepted { "rejected" } else { "unknown" }
        );
        assert_eq!(app.writes().len(), 1);
    }
}
#[tokio::test]
async fn uncertain_patch_and_lost_confirmation_keep_shared_fence_across_new_keys() {
    for lost_receipt in [false, true] {
        let app = App::new(Some(11), false).await;
        if lost_receipt {
            let backend = app.backend.clone();
            let mut memory = app.memory.lock().unwrap();
            memory.hook_path = "/api/v10/channels/99".into();
            memory.hook_method = "PATCH".into();
            let flag = backend.fail_put.clone();
            memory.hook = Some(Box::new(move || {
                flag.store(true, Ordering::SeqCst);
            }));
            drop(memory);
            let value = body(app.manage(app.target(), "archive", "first").await).await;
            backend.fail_put.store(false, Ordering::SeqCst);
            assert_eq!(value["outcome"]["status"], "unknown");
        } else {
            app.memory.lock().unwrap().status = StatusCode::BAD_GATEWAY;
            assert_eq!(
                body(app.manage(app.target(), "archive", "first").await).await["outcome"]["status"],
                "unknown"
            );
        }
        assert_eq!(
            body(app.manage(app.target(), "reopen", "second").await).await["outcome"]["status"],
            "unknown"
        );
        assert_eq!(app.writes().len(), 1);
    }
}
#[tokio::test]
async fn abandoned_claim_and_fence_survive_without_lease_takeover() {
    let app = App::new(Some(11), false).await;
    let store = ledger::Store::new(app.state.resolver.record_client(), app.state.keys.clone());
    let owner = &app.definition.owner.subject;
    let claim = store
        .claim(
            "retained-scope".into(),
            "key",
            "request".into(),
            owner,
            app.event.expires_at,
        )
        .await
        .unwrap();
    assert!(matches!(claim, ledger::Admission::Claimed(_)));
    let fresh = ledger::Store::new(app.state.resolver.record_client(), app.state.keys.clone());
    let replay = fresh
        .claim(
            "retained-scope".into(),
            "key",
            "request".into(),
            owner,
            app.event.expires_at,
        )
        .await
        .unwrap();
    assert!(matches!(
        replay,
        ledger::Admission::Replay(ThreadReceipt {
            outcome: ThreadOutcome::Unknown { .. },
            ..
        })
    ));
    let (left, right) = tokio::join!(
        store.acquire("77", "99", owner),
        fresh.acquire("77", "99", owner)
    );
    assert_eq!(
        usize::from(left.unwrap().is_some()) + usize::from(right.unwrap().is_some()),
        1
    );
    assert_eq!(
        body(app.manage(app.target(), "archive", "new-key").await).await["outcome"]["status"],
        "unknown"
    );
    assert!(app.writes().is_empty());
    app.backend.expire_at(app.event.expires_at + 1);
    assert!(fresh.acquire("77", "99", owner).await.unwrap().is_none());
}
#[tokio::test]
async fn revocation_after_preflight_stops_writes_for_default_and_named_consumers() {
    for named in [false, true] {
        for case in 0..5 {
            let app = App::new(Some(11), named).await;
            app.change_on_read("/api/v10/channels/99", |definition| match case {
                0 => {
                    let Source::Discord {
                        thread_lifecycle, ..
                    } = &mut definition.sources[0]
                    else {
                        panic!()
                    };
                    *thread_lifecycle = false;
                }
                1 => definition.secrets.discord_bot_token = Some(Secret::new("changed-token")),
                2 if named => {
                    definition.routing.as_mut().unwrap().consumers[0]
                        .provider
                        .binding_id = "rebound".parse().unwrap()
                }
                2 => definition.provider.binding_id = "rebound".parse().unwrap(),
                3 if named => {
                    definition.secrets.consumer_keys[0].1 = Secret::new("revoked-key");
                    definition.routing.as_mut().unwrap().consumers[0].agent_key_hash =
                        crate::channels::definition::hash_secret("revoked-key");
                }
                3 => crate::channels::definition::fixture::replace_default_agent_key(
                    definition,
                    "revoked-key",
                ),
                _ if named => definition.routing.as_mut().unwrap().consumers[0].pending = true,
                _ => definition.secrets.provider_credential = None,
            });
            let value = body(app.manage(app.target(), "archive", "mutation").await).await;
            assert_eq!(
                value["outcome"]["status"], "rejected",
                "named={named} case={case}: {value}"
            );
            assert!(app.writes().is_empty());
        }
    }
}
#[tokio::test]
async fn list_rechecks_custody_after_platform_read_and_never_returns_revoked_refs() {
    let app = App::new(None, true).await;
    app.change_on_read("/api/v10/guilds/100/threads/active", |definition| {
        definition.secrets.discord_bot_token = Some(Secret::new("rotated"))
    });
    assert_eq!(app.list("active").await.status(), 403);
    assert!(app.writes().is_empty());
}
#[tokio::test]
async fn altered_event_consumer_actor_parent_or_expiry_never_grants_a_thread_reference() {
    for case in 0..6 {
        let mut app = App::new(Some(11), true).await;
        match case {
            0 => app.event.consumer = None,
            1 => app.event.partition_key = "foreign".into(),
            2 => app.event.activity_sha256 = Some("a".repeat(64)),
            3 => app.event.expires_at += 1,
            4 => {
                app.memory.lock().unwrap().channels.get_mut("99").unwrap()["parent_id"] =
                    "97".into()
            }
            _ => {
                app.memory.lock().unwrap().channels.get_mut("99").unwrap()["guild_id"] =
                    "200".into()
            }
        }
        assert!(
            !app.list("active").await.status().is_success(),
            "case {case}"
        );
        assert!(app.writes().is_empty());
    }
}
#[tokio::test]
async fn list_issued_reference_cannot_transfer_to_an_unrelated_consumer_or_changed_credentials() {
    for named in [false, true] {
        let mut app = App::new(None, named).await;
        let value = body(app.list("active").await).await;
        let target = json!({"type":"thread","thread_ref":value["threads"][0]["thread_ref"]});
        let key = app.key.clone();
        app.key = if named {
            fixture::DEFAULT_KEY.into()
        } else {
            crate::channels::routing_fixture::agent_key(crate::channels::routing_fixture::REVIEWER)
        };
        assert_eq!(
            app.manage(target.clone(), "archive", "foreign-key")
                .await
                .status(),
            403
        );
        app.key = key;
        app.definition.secrets.discord_bot_token = Some(Secret::new("rotated"));
        app.save();
        assert_eq!(
            app.manage(target, "archive", "changed-custody")
                .await
                .status(),
            403
        );
        assert!(app.writes().is_empty());
    }
}
#[tokio::test]
async fn mcp_and_rest_share_consent_and_exact_operations_and_the_catalog_gate() {
    let mut app = App::new(Some(11), false).await;
    let invoke = |name: &str, arguments: Value| json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":arguments}});
    let response = reqwest::Client::new()
        .post(format!("{}/mcp", app.base))
        .bearer_auth(&app.key)
        .json(&invoke(
            "manageThread",
            json!({"request":{"target":app.target(),"action":"join"},"idempotency_key":"mcp-join"}),
        ))
        .send()
        .await
        .unwrap();
    let value = body(response).await;
    assert!(value.to_string().contains("confirmed"), "{value}");
    let catalog = crate::mcp::catalog::view(&app.state);
    assert!(catalog.iter().any(|tool| tool.name == "manageThread"));
    app.state.discord_threads = None;
    app.mount().await;
    assert!(!crate::mcp::catalog::view(&app.state)
        .iter()
        .any(|tool| tool.name == "manageThread"));
    assert_eq!(app.list("active").await.status(), 403);
    assert_eq!(
        app.manage(app.target(), "join", "gate-off").await.status(),
        403
    );
    assert_eq!(app.writes().len(), 1);
}

#[tokio::test]
async fn announcement_create_uses_announcement_thread_type_and_voice_has_no_lifecycle_scope() {
    let app = App::with_origin(5, false).await;
    let value = body(app.create("announcement").await).await;
    assert_eq!(value["outcome"]["status"], "confirmed");
    let reference: ThreadRef =
        serde_json::from_value(value["thread"]["thread_ref"].clone()).unwrap();
    assert_eq!(
        reference
            .open(&app.state.keys, now())
            .unwrap()
            .payload
            .thread_type,
        10
    );
    let app = App::with_origin(2, false).await;
    assert_eq!(app.list("active").await.status(), 400);
    assert_eq!(app.create("voice").await.status(), 400);
    assert!(app.memory.lock().unwrap().calls.is_empty());
}

#[tokio::test]
async fn creation_rechecks_the_exact_human_message_before_any_write() {
    for case in 0..7 {
        let app = App::new(None, false).await;
        {
            let mut memory = app.memory.lock().unwrap();
            match case {
                0 => memory.message["author"]["id"] = "52".into(),
                1 => memory.message["author"]["bot"] = true.into(),
                2 => memory.message["author"]["system"] = true.into(),
                3 => memory.message["author"]["bot"] = "false".into(),
                4 => memory.message["webhook_id"] = "88".into(),
                5 => memory.message["type"] = 6.into(),
                _ => memory.message["channel_id"] = "97".into(),
            }
        }
        let result = body(app.create("exact-human").await).await;
        assert_eq!(result["outcome"]["status"], "rejected", "case {case}");
        assert!(app.writes().is_empty());
    }
}

#[tokio::test]
async fn context_capabilities_require_retained_scope_and_do_not_grant_history() {
    for threaded in [false, true] {
        let app = App::new(threaded.then_some(11), true).await;
        let response = reqwest::Client::new()
            .post(format!(
                "{}/v1/events/{}/context",
                app.base,
                app.reference().as_str()
            ))
            .bearer_auth(&app.key)
            .json(&app.activity)
            .send()
            .await
            .unwrap();
        let value = body(response).await;
        let capabilities = &value["capabilities"]["thread_lifecycle"];
        assert_eq!(capabilities["create_from_original_message"], !threaded);
        assert_eq!(capabilities["manage_original_thread"], threaded);
        assert_eq!(capabilities["expires_at"], app.event.expires_at);
        assert!(value["capabilities"].get("thread_history").is_none());
        assert!(app.memory.lock().unwrap().calls.is_empty());
    }
}

#[tokio::test]
async fn listed_public_target_is_revalidated_before_mutation_and_never_becomes_private_authority() {
    for case in 0..5 {
        let app = App::new(None, false).await;
        let list = body(app.list("active").await).await;
        let mut changed = thread("701", 11, "98", false);
        match case {
            0 => changed["type"] = 12.into(),
            1 => changed["parent_id"] = "97".into(),
            2 => changed["guild_id"] = "200".into(),
            3 => changed["id"] = "88".into(),
            _ => {
                changed.as_object_mut().unwrap().remove("thread_metadata");
            }
        }
        app.memory
            .lock()
            .unwrap()
            .channels
            .insert("701".into(), changed);
        let target = json!({"type":"thread","thread_ref":list["threads"][0]["thread_ref"]});
        let result = body(app.manage(target, "archive", "listed-target").await).await;
        assert_eq!(result["outcome"]["status"], "rejected", "case {case}");
        assert!(app.writes().is_empty());
    }
}
#[tokio::test]
async fn a_read_can_confirm_unknown_creation_only_with_exact_owner_name_duration_and_identity() {
    let expected = Identity {
        id: "700".into(),
        kind: 11,
        guild: "100".into(),
        parent: Some("98".into()),
    };
    for case in 0..6 {
        let platform = crate::discord::rest_fixture::Fixture::new().await;
        let mut value = thread("700", 11, "98", false);
        match case {
            0 => value["owner_id"] = "51".into(),
            1 => value["name"] = "Different".into(),
            2 => value["thread_metadata"]["auto_archive_duration"] = 60.into(),
            3 => value["id"] = "88".into(),
            4 => value["parent_id"] = "97".into(),
            _ => value["type"] = 12.into(),
        }
        platform.respond(StatusCode::OK, value);
        assert!(platform
            .api
            .reconcile_created_thread(
                &Secret::new("fixture"),
                &expected,
                "77",
                &CreateThreadRequest {
                    name: "Review".into(),
                    auto_archive_duration: 1440
                }
            )
            .await
            .is_err());
        let calls = platform.memory.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].method, "GET");
        assert_eq!(calls[0].path, "/api/v10/channels/700");
    }
}
