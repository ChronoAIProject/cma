use super::*;
use cmaeg_core::channel::{LarkRegion, Secret, Source};
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
#[path = "fixture.rs"]
mod fixture;
#[path = "mixed.rs"]
mod mixed;
use fixture::{row, Fixture};
fn cursor(page: ConversationHistoryPage) -> HistoryCursor {
    let HistoryContinuation::MoreVisibleRows { cursor } = page.continuation else {
        panic!("expected explicit continuation")
    };
    cursor
}
#[tokio::test]
async fn native_history_chat_and_exact_thread_regions_and_consumers() {
    for region in [LarkRegion::Lark, LarkRegion::Feishu] {
        for named in [false, true] {
            for threaded in [false, true] {
                let f = Fixture::new(region, named, threaded, true).await;
                f.rows(2, None);
                f.app.backend.fail_put.store(true, Ordering::SeqCst);
                let page = f.read(None).await.unwrap();
                assert_eq!(page.messages.len(), 2);
                assert_eq!(page.messages[0].message_id, "om_row_1");
                assert_eq!(page.conversation, f.app.activity.conversation);
                assert_eq!(page.expires_at, f.app.event.expires_at);
                assert_eq!(f.list_calls(), 1);
                assert_eq!(page.before_message_id, "om_original");
                let calls = f.api.calls.lock().unwrap();
                assert_eq!(calls.len(), 5);
                assert!(calls
                    .iter()
                    .all(|(_, u)| u.origin().ascii_serialization() == region.origin()));
                assert_eq!(
                    calls
                        .iter()
                        .filter(|(_, u)| u.path().ends_with("/om_original"))
                        .count(),
                    2
                );
            }
        }
    }
}
#[tokio::test]
async fn native_history_thread_scans_newer_rows_with_empty_bounded_progress() {
    let f = Fixture::new(LarkRegion::Feishu, false, true, true).await;
    f.set_page(
        "",
        (0..25)
            .map(|i| row(&format!("om_new_{i}"), f.created + 1000 - i, true))
            .collect(),
        Some("opaque/+?one"),
    );
    f.set_page(
        "opaque/+?one",
        vec![
            row("om_equal", f.created, true),
            row("om_original", f.created, true),
            row("om_old", f.created - 1, true),
        ],
        Some("last"),
    );
    f.set_page("last", vec![row("om_oldest", f.created - 2, true)], None);
    let page = f.read(None).await.unwrap();
    assert_eq!(page.scanned_messages, 25);
    assert_eq!(page.omitted_message_count, 25);
    assert!(page.messages.is_empty());
    let c = cursor(page);
    let opened = c
        .open_native_lark(&f.app.state.keys, now())
        .unwrap()
        .payload;
    assert_eq!(opened.before_message_id, "om_original");
    assert_eq!(opened.expires_at, f.app.event.expires_at);
    assert_eq!(f.list_calls(), 1);
    let page = f.read(Some(&c)).await.unwrap();
    assert_eq!(page.before_message_id, "om_original");
    assert_eq!(page.omitted_message_count, 2);
    assert_eq!(page.messages[0].message_id, "om_old");
    let page = f.read(Some(&cursor(page))).await.unwrap();
    assert_eq!(page.before_message_id, "om_old");
    assert_eq!(page.messages[0].message_id, "om_oldest");
    assert_eq!(page.continuation, HistoryContinuation::EndOfVisibleHistory);
    assert_eq!(f.list_calls(), 3);
    let calls = f.api.calls.lock().unwrap();
    assert!(calls
        .iter()
        .filter(|(_, u)| u.path() == "/open-apis/im/v1/messages")
        .all(|(_, u)| !u
            .query_pairs()
            .any(|(k, _)| k == "end_time" || k == "start_time")));
}
#[tokio::test]
async fn native_history_chat_cutoff_and_continuation_never_order_message_ids() {
    let f = Fixture::new(LarkRegion::Lark, false, false, true).await;
    f.set_page(
        "",
        vec![
            row("om_a", f.created + 100, false),
            row("om_original", f.created, false),
            row("om_b", f.created, false),
            row("om_zzz", f.created - 1, false),
        ],
        Some("next"),
    );
    f.set_page("next", vec![row("om_aaa", f.created - 2, false)], None);
    let page = f.read(None).await.unwrap();
    assert_eq!(page.omitted_message_count, 3);
    assert_eq!(page.messages[0].message_id, "om_zzz");
    let page = f.read(Some(&cursor(page))).await.unwrap();
    assert_eq!(page.messages[0].message_id, "om_aaa");
    assert_eq!(page.before_message_id, "om_zzz");
    for (_, u) in f
        .api
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, u)| u.path() == "/open-apis/im/v1/messages")
    {
        let q: std::collections::BTreeMap<_, _> = u.query_pairs().into_owned().collect();
        assert_eq!(q["end_time"], (f.created / 1000 + 1).to_string());
    }
}
#[tokio::test]
async fn native_history_consent_cannot_be_acquired_retroactively_and_gate_is_separate() {
    let mut f = Fixture::new(LarkRegion::Feishu, false, false, false).await;
    assert!(f.read(None).await.is_err());
    let Source::NativeLark(s) = &mut f.app.definition.sources[0] else {
        panic!()
    };
    s.conversation_history = true;
    f.app.replace();
    assert!(f.read(None).await.is_err());
    assert!(f.api.calls.lock().unwrap().is_empty());
    let mut f = Fixture::new(LarkRegion::Feishu, false, true, true).await;
    let channel = f
        .app
        .state
        .resolver
        .resolve_current(f.app.event.channel_id.as_str())
        .await
        .unwrap();
    assert!(f
        .app
        .state
        .native_history_capabilities(f.app.caller(), &f.app.event, &channel)
        .await
        .is_some());
    assert!(f.api.calls.lock().unwrap().is_empty());
    f.app.state.native_lark_conversation_history = None;
    assert!(f
        .app
        .state
        .native_history_capabilities(f.app.caller(), &f.app.event, &channel)
        .await
        .is_none());
    assert_eq!(f.read(None).await.unwrap_err().status, 403);
    assert!(f.api.calls.lock().unwrap().is_empty());
}
#[tokio::test]
async fn native_history_admission_custody_survives_no_key_or_partition_rebinding() {
    for case in 0..7 {
        let f = Fixture::new(LarkRegion::Feishu, false, true, true).await;
        f.change_on_read(move |d| match case {
            0 => {
                let Source::NativeLark(s) = &mut d.sources[0] else {
                    panic!()
                };
                s.conversation_history = false;
            }
            1 => {
                d.secrets.native_lark.as_mut().unwrap().app_secret =
                    Secret::new("rotated-app-secret-123456")
            }
            2 => crate::channels::definition::fixture::replace_default_agent_key(
                d,
                "changed-agent-key",
            ),
            3 => d.owner.subject = "changed-owner".into(),
            4 => crate::channels::definition::fixture::replace_default_credential(
                d,
                Some("changed-provider-token"),
            ),
            5 => crate::channels::definition::fixture::replace_default_binding(
                d,
                "rebound".parse().unwrap(),
            ),
            _ => d.partition.version += 1,
        });
        assert!(f.read(None).await.is_err(), "race {case}");
        assert_eq!(f.list_calls(), 1);
    }
    let mut f = Fixture::new(LarkRegion::Feishu, false, false, true).await;
    crate::channels::definition::fixture::replace_default_agent_key(
        &mut f.app.definition,
        "changed-agent-key",
    );
    f.app.key = "changed-agent-key".into();
    f.app.replace();
    assert!(
        f.read(None).await.is_err(),
        "new key must not acquire an old admission"
    );
    assert!(f.api.calls.lock().unwrap().is_empty());
}
#[tokio::test]
async fn native_history_anchor_and_exact_container_are_rechecked() {
    for case in 0..9 {
        let f = Fixture::new(LarkRegion::Feishu, false, true, true).await;
        let mut anchor = f.api.anchor.lock().unwrap().clone();
        match case {
            0 => anchor["deleted"] = true.into(),
            1 => anchor["chat_id"] = "oc_other".into(),
            2 => anchor["thread_id"] = "omt_other".into(),
            3 => anchor["sender"]["id"] = "ou_other".into(),
            4 => anchor["sender"]["tenant_key"] = "other".into(),
            5 => anchor["create_time"] = (f.created - 1).to_string().into(),
            6 => anchor["message_id"] = "om_other".into(),
            7 => anchor["sender"]["sender_type"] = "app".into(),
            _ => anchor["sender"]["id_type"] = "user_id".into(),
        }
        *f.api.final_anchor.lock().unwrap() = Some(anchor);
        assert!(f.read(None).await.is_err(), "anchor {case}");
    }
    for field in ["thread_id", "chat_id"] {
        let f = Fixture::new(LarkRegion::Lark, false, true, true).await;
        let mut r = row("om_other", f.created - 1, true);
        r[field] = if field == "thread_id" {
            "omt_other"
        } else {
            "oc_other"
        }
        .into();
        f.set_page("", vec![r], None);
        assert!(f.read(None).await.is_err());
    }
    let f = Fixture::new(LarkRegion::Lark, false, false, true).await;
    f.api.anchor.lock().unwrap()["body"] = json!({"content":"edited original"});
    assert!(f.read(None).await.is_ok());
}
#[tokio::test]
async fn native_history_cursors_are_original_custody_expiry_and_platform_bound() {
    let f = Fixture::new(LarkRegion::Feishu, false, true, true).await;
    f.rows(25, Some("next"));
    let c = cursor(f.read(None).await.unwrap());
    let original = c
        .open_native_lark(&f.app.state.keys, now())
        .unwrap()
        .payload;
    assert!(c.open(&f.app.state.keys, now()).is_err());
    for case in 0..7 {
        let mut p = original.clone();
        match case {
            0 => p.event_sha256 = "0".repeat(64),
            1 => p.custody = "0".repeat(64),
            2 => p.origin.identity.region = LarkRegion::Lark,
            3 => p.origin.chat_id = "oc_other".into(),
            4 => p.origin.thread_id = Some("omt_other".into()),
            5 => p.anchor_created_ms -= 1,
            _ => p.expires_at += 1,
        }
        let altered = HistoryCursor::seal_native_lark(&p, &f.app.state.keys).unwrap();
        assert!(f.read(Some(&altered)).await.is_err());
    }
    let mut expired = original.clone();
    expired.issued_at = now() - 2;
    expired.expires_at = now() - 1;
    let expired = HistoryCursor::seal_native_lark(&expired, &f.app.state.keys).unwrap();
    assert!(f.read(Some(&expired)).await.is_err());
    assert_eq!(f.list_calls(), 1);
    let other = Fixture::new(LarkRegion::Feishu, true, true, true).await;
    assert!(other.read(Some(&c)).await.is_err());
    assert!(other.api.calls.lock().unwrap().is_empty());
}
#[tokio::test]
async fn native_history_provider_pages_validate_progress_and_response_limits() {
    for case in 0..9 {
        let f = Fixture::new(LarkRegion::Feishu, false, false, true).await;
        let mut value = fixture::page(vec![row("om_a", f.created - 1, false)], Some("next"));
        match case {
            0 => value["data"]["page_token"] = Value::Null,
            1 => value["data"]["page_token"] = "x".repeat(1025).into(),
            2 => {
                value["data"]["items"] = json!((0..26)
                    .map(|i| row(&format!("om_{i}"), f.created - 1 - i, false))
                    .collect::<Vec<_>>())
            }
            3 => {
                value["data"]["items"] = json!([
                    row("om_a", f.created - 1, false),
                    row("om_a", f.created - 2, false)
                ])
            }
            4 => {
                value["data"]["items"] = json!([
                    row("om_a", f.created - 2, false),
                    row("om_b", f.created - 1, false)
                ])
            }
            5 => value["data"]["items"][0]["sender"]["tenant_key"] = "other_tenant".into(),
            6 => value["data"]["has_more"] = "true".into(),
            7 => value["data"]["items"][0]["body"]["content"] = "x".repeat(1024 * 1024).into(),
            _ => value["code"] = 230073.into(),
        }
        f.api.pages.lock().unwrap().insert("".into(), value);
        assert!(f.read(None).await.is_err(), "malformed {case}");
        assert_eq!(f.list_calls(), 1);
    }
    let f = Fixture::new(LarkRegion::Lark, false, false, true).await;
    f.rows(1, Some("next"));
    let c = cursor(f.read(None).await.unwrap());
    f.set_page("next", vec![], Some("next"));
    assert!(f.read(Some(&c)).await.is_err());
    let f = Fixture::new(LarkRegion::Lark, false, false, true).await;
    f.rows(1, None);
    *f.api.status.lock().unwrap() = reqwest::StatusCode::TOO_MANY_REQUESTS;
    assert_eq!(f.read(None).await.unwrap_err().status, 429);
    assert_eq!(f.list_calls(), 1);
}
#[tokio::test]
async fn native_history_omissions_and_text_bounds_create_no_media_authority() {
    let f = Fixture::new(LarkRegion::Feishu, false, true, true).await;
    let mut rows = vec![];
    for (i, kind) in [
        "image",
        "file",
        "audio",
        "media",
        "interactive",
        "merge_forward",
        "system",
        "sticker",
        "unknown",
    ]
    .iter()
    .enumerate()
    {
        let mut r = row(
            &format!("om_unsupported_{i}"),
            f.created - 1 - i as i64,
            true,
        );
        r["msg_type"] = (*kind).into();
        r["body"] = json!({"content":"unparsed unsupported body"});
        rows.push(r);
    }
    let mut deleted = row("om_deleted", f.created - 10, true);
    deleted["deleted"] = true.into();
    deleted.as_object_mut().unwrap().remove("body");
    rows.push(deleted);
    for i in 0..5 {
        let mut r = row(&format!("om_text_{i}"), f.created - 20 - i, true);
        r["body"]["content"] = json!({"text":"好".repeat(5000)}).to_string().into();
        rows.push(r);
    }
    f.set_page("", rows, None);
    let page = f.read(None).await.unwrap();
    assert_eq!(page.omitted_message_count, 10);
    assert_eq!(page.messages.len(), 5);
    assert!(page.text_truncated);
    assert!(page.messages.iter().map(|m| m.text.len()).sum::<usize>() <= 32 * 1024);
    assert!(page
        .messages
        .iter()
        .all(|m| m.text.len() <= 8192 && m.attachments.is_empty()));
    let wire = serde_json::to_string(&page).unwrap();
    assert!(!wire.contains("resource_key"));
    assert!(!wire.contains("message_ref"));
    let mut large = row("om_large", f.created - 1, true);
    large["body"]["content"] = json!({"text":"x".repeat(100_000)}).to_string().into();
    f.set_page("", vec![large], None);
    assert!(
        f.read(None).await.unwrap().text_truncated,
        "history reader supports bounded responses beyond ordinary 64 KiB"
    );
}
#[tokio::test]
async fn native_history_busy_deadline_and_original_expiry_fail_closed() {
    let mut f = Fixture::new(LarkRegion::Feishu, false, true, true).await;
    let client = f
        .app
        .state
        .native_lark_conversation_history
        .as_ref()
        .unwrap();
    let held = client.slots.acquire_many(8).await.unwrap();
    assert_eq!(f.read(None).await.unwrap_err().status, 429);
    drop(held);
    assert!(f.api.calls.lock().unwrap().is_empty());
    Arc::get_mut(
        f.app
            .state
            .native_lark_conversation_history
            .as_mut()
            .unwrap(),
    )
    .unwrap()
    .budget = Duration::from_secs(1);
    f.rows(1, Some("never_issued_after_timeout"));
    f.api.delay.store(5000, Ordering::SeqCst);
    let (result, started) = tokio::join!(
        f.read(None),
        tokio::time::timeout(Duration::from_secs(2), f.api.list_started.notified())
    );
    started.expect("the stalled provider request must actually start");
    assert_eq!(result.unwrap_err().status, 503);
    assert_eq!(f.list_calls(), 1);
    assert_eq!(f.api.cancelled_lists.load(Ordering::SeqCst), 1);
    assert_eq!(
        f.api.calls.lock().unwrap().len(),
        4,
        "no final anchor or extra request after timeout"
    );
    let f = Fixture::until(LarkRegion::Feishu, false, true, true, now() + 2).await;
    f.rows(1, Some("never_issued_after_expiry"));
    f.api.delay.store(2100, Ordering::SeqCst);
    assert_eq!(f.read(None).await.unwrap_err().status, 410);
}
#[tokio::test]
async fn native_history_http_and_mcp_keep_one_page_and_reject_destination_inputs() {
    let f = Fixture::new(LarkRegion::Lark, true, true, true).await;
    f.rows(1, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let router = crate::http::tools::router(f.app.state.clone())
        .merge(crate::mcp::router(f.app.state.clone()));
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let http = reqwest::Client::new();
    let catalog: Value = http
        .post(format!("{base}/mcp"))
        .bearer_auth(&f.app.key)
        .json(&json!({"jsonrpc":"2.0","id":0,"method":"tools/list","params":{}}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        catalog["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == "readEventHistory"),
        "{catalog}"
    );
    let path = format!("{base}/v1/events/{}/history", f.reference().as_str());
    for extra in [
        "chat_id=oc_other",
        "thread_id=omt_other",
        "limit=50",
        "cursor=a&cursor=b",
    ] {
        let r = http
            .get(format!("{path}?{extra}"))
            .bearer_auth(&f.app.key)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 400);
    }
    let response = http
        .get(&path)
        .bearer_auth(&f.app.key)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let page: ConversationHistoryPage = response.json().await.unwrap();
    assert_eq!(page.messages.len(), 1);
    assert_eq!(f.list_calls(), 1);
    let response=http.post(format!("{base}/mcp")).bearer_auth(&f.app.key).json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"readEventHistory","arguments":{"event_ref":f.reference()}}})).send().await.unwrap();
    let result: Value = response.json().await.unwrap();
    assert!(result.get("error").is_none(), "{result}");
    assert_eq!(f.list_calls(), 2);
    let r = http
        .get(&path)
        .bearer_auth("nyxid_ag_wrong_consumer")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    assert_eq!(f.list_calls(), 2);
    server.abort();
}
#[test]
fn native_history_consent_preserves_pinned_messaging_revision() {
    let Source::NativeLark(source) = crate::native_lark::test_support::source() else {
        panic!()
    };
    let mut value = serde_json::to_value(&source).unwrap();
    assert!(value.get("conversation_history").is_none());
    let old =
        crate::native_lark::revision(&source, &crate::native_lark::test_support::credentials());
    // SHA-256 of the exact pre-history typed source/credential tuple, without a history field.
    assert_eq!(
        old,
        "93b31d16040cd2daccb2f5211163d696517704c56ca1adce6bd2103a1ff6f13b"
    );
    value["conversation_history"] = false.into();
    let same = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(
        old,
        crate::native_lark::revision(&same, &crate::native_lark::test_support::credentials())
    );
    value["conversation_history"] = true.into();
    let changed = serde_json::from_value(value).unwrap();
    assert_eq!(
        old,
        crate::native_lark::revision(&changed, &crate::native_lark::test_support::credentials())
    );
}

#[tokio::test]
async fn native_history_missing_admission_and_bot_mismatch_refuse_before_listing() {
    let f = Fixture::new(LarkRegion::Lark, true, true, true).await;
    f.app.backend.expire_at(i64::MAX);
    assert!(f.read(None).await.is_err());
    assert!(f.api.calls.lock().unwrap().is_empty());

    let f = Fixture::new(LarkRegion::Feishu, false, false, true).await;
    *f.api.bot.lock().unwrap() = "ou_different_bot".into();
    assert!(f.read(None).await.is_err());
    assert_eq!(f.list_calls(), 0);
    assert_eq!(f.api.calls.lock().unwrap().len(), 2);

    let mut f = Fixture::new(LarkRegion::Lark, false, true, true).await;
    f.app.event.lineage = Some(
        serde_json::from_value(json!({
            "root":"0123456789abcdef0123456789abcdef", "hop":1,
            "path":["default"], "reply":"origin"
        }))
        .unwrap(),
    );
    assert!(f.read(None).await.is_err());
    assert!(f.api.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn native_history_app_posts_and_unsupported_actors_are_honest_projection() {
    let f = Fixture::new(LarkRegion::Feishu, false, true, true).await;
    let mut post = row("om_post", f.created - 1, true);
    post["msg_type"] = "post".into();
    post["sender"] = json!({"sender_type":"app", "id_type":"app_id",
        "id":"cli_other_app", "tenant_key":"native_tenant"});
    post["updated"] = true.into();
    post["update_time"] = f.created.to_string().into();
    post["body"]["content"] = json!({"title":"Earlier post", "content":[[
        {"tag":"text","text":"Context\u{0000} and "},
        {"tag":"a","text":"source","href":"https://example.test/reference"}
    ]]})
    .to_string()
    .into();
    let mut media = row("om_media_post", f.created - 2, true);
    media["msg_type"] = "post".into();
    media["body"]["content"] = json!({"title":"Media", "content":[[
        {"tag":"img","image_key":"img_history_only"}
    ]]})
    .to_string()
    .into();
    let mut anonymous = row("om_anonymous", f.created - 3, true);
    anonymous["sender"] = json!({"sender_type":"anonymous"});
    let mut forwarded = row("om_forwarded_child", f.created - 4, true);
    forwarded["upper_message_id"] = "om_forwarded_parent".into();
    f.set_page("", vec![post, media, anonymous, forwarded], None);
    let page = f.read(None).await.unwrap();
    assert_eq!(page.scanned_messages, 4);
    assert_eq!(page.omitted_message_count, 3);
    assert_eq!(page.messages.len(), 1);
    let post = &page.messages[0];
    assert_eq!(post.author.kind, cmaeg_core::activity::ActorKind::Bot);
    assert_eq!(post.author.id, "cli_other_app");
    assert!(post.edited_at.is_some());
    assert!(post
        .text
        .contains("source (https://example.test/reference)"));
    assert!(!post.text.contains('\0'));
    assert!(post.attachments.is_empty());
    assert!(!serde_json::to_string(&page)
        .unwrap()
        .contains("img_history_only"));
}
