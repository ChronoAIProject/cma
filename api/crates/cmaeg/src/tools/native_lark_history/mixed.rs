use super::*;
use cmaeg_core::conversation_history::HistoryOmittedContent;

pub(super) fn enable(f: &mut Fixture) {
    Arc::get_mut(
        f.app
            .state
            .native_lark_conversation_history
            .as_mut()
            .unwrap(),
    )
    .unwrap()
    .mixed_media = true;
}

pub(super) fn post(id: &str, created: i64, threaded: bool, title: &str, items: Value) -> Value {
    let mut row = row(id, created, threaded);
    row["msg_type"] = "post".into();
    row["body"]["content"] = json!({"title":title, "content":[items]}).to_string().into();
    row
}

#[tokio::test]
async fn native_history_mixed_posts_preserve_text_and_report_media_without_authority() {
    for region in [LarkRegion::Lark, LarkRegion::Feishu] {
        for threaded in [false, true] {
            let mut f = Fixture::new(region, false, threaded, true).await;
            let mixed = post(
                "om_mixed",
                f.created - 1,
                threaded,
                "Earlier post",
                json!([
                    {"tag":"text","text":"Before\u{0000} "},
                    {"tag":"img","image_key":"img_history_only"},
                    {"tag":"text","text":" after "},
                    {"tag":"media","file_key":"file_history_only","image_key":"img_thumbnail_only"},
                    {"tag":"code_block","text":"code"},
                    {"tag":"a","text":" source","href":"https://example.test/reference"}
                ]),
            );
            f.set_page("", vec![mixed], Some("next_page"));
            let old = f.read(None).await.unwrap();
            assert!(old.messages.is_empty());
            assert_eq!(old.omitted_message_count, 1);
            let old_cursor = cursor(old);
            enable(&mut f);
            let page = f.read_with_media(None, true).await.unwrap();
            let mixed_cursor = cursor(page.clone());
            assert_eq!(page.scanned_messages, 1);
            assert_eq!(page.omitted_message_count, 0);
            assert_eq!(page.messages.len(), 1);
            let message = &page.messages[0];
            assert_eq!(
                message.text,
                "Earlier postBefore  after code source (https://example.test/reference)"
            );
            assert_eq!(message.omitted_content, [HistoryOmittedContent::Media]);
            assert!(message.attachments.is_empty());
            assert!(
                message.text_truncated,
                "existing control/newline sanitization is reported"
            );
            assert!(!message.metadata_truncated);
            assert_eq!(page.expires_at, f.app.event.expires_at);
            let wire = serde_json::to_string(&page).unwrap();
            for forbidden in [
                "img_history_only",
                "file_history_only",
                "img_thumbnail_only",
                "resource_key",
                "message_ref",
                "file_ref",
            ] {
                assert!(!wire.contains(forbidden), "{forbidden}");
            }
            assert_eq!(f.list_calls(), 2);
            assert_eq!(
                f.api.calls.lock().unwrap().len(),
                10,
                "no media fetch or automatic next page"
            );
            // Old cursors keep their legacy projection even after activation.
            f.set_page(
                "next_page",
                vec![post(
                    "om_older",
                    f.created - 2,
                    threaded,
                    "Older",
                    json!([
                        {"tag":"img","image_key":"img_older_only"}
                    ]),
                )],
                None,
            );
            let next = f.read(Some(&old_cursor)).await.unwrap();
            assert!(next.messages.is_empty());
            assert_eq!(next.omitted_message_count, 1);
            let next = f.read_with_media(Some(&mixed_cursor), true).await.unwrap();
            assert_eq!(next.messages[0].text, "Older");
            assert_eq!(
                next.messages[0].omitted_content,
                [HistoryOmittedContent::Media]
            );
            assert_eq!(next.before_message_id, "om_mixed");
            assert_eq!(next.expires_at, f.app.event.expires_at);
        }
    }
}

#[tokio::test]
async fn native_history_mixed_posts_keep_omission_when_text_budget_is_exhausted() {
    let mut f = Fixture::new(LarkRegion::Feishu, true, true, true).await;
    enable(&mut f);
    let rows = (0..7)
        .map(|i| {
            post(
                &format!("om_mixed_{i}"),
                f.created - 1 - i,
                true,
                "Title",
                json!([
                    {"tag":"text","text":"好".repeat(5000)},
                    {"tag":"img","image_key":"img_bounded"}
                ]),
            )
        })
        .collect();
    f.set_page("", rows, None);
    let page = f.read_with_media(None, true).await.unwrap();
    assert_eq!(page.scanned_messages, 7);
    assert_eq!(page.omitted_message_count, 0);
    assert_eq!(page.messages.len(), 7);
    assert!(page.text_truncated);
    assert!(page.messages.iter().any(|m| m.text.is_empty()));
    assert!(page.messages.iter().map(|m| m.text.len()).sum::<usize>() <= 32 * 1024);
    for message in &page.messages {
        assert!(message.text.len() <= 8192);
        assert!(message.text_truncated);
        assert_eq!(message.omitted_content, [HistoryOmittedContent::Media]);
        assert!(message.attachments.is_empty());
    }
    assert_eq!(f.list_calls(), 1);
}

#[tokio::test]
async fn native_history_mixed_projection_still_refuses_malformed_supported_posts() {
    let mut f = Fixture::new(LarkRegion::Lark, false, false, true).await;
    enable(&mut f);
    for items in [
        json!([{"tag":"img"}]),
        json!([{"tag":"media","file_key":"file_valid","image_key":"invalid key"}]),
        json!([{"tag":"text","text":"Visible"},{"tag":"unknown","text":"Hidden"}]),
        json!([{"tag":"text","text":"x".repeat(32 * 1024 + 1)}]),
        json!(vec![
            json!({"tag":"img","image_key":"img_repeated"});
            cmaeg_core::activity::MAX_ATTACHMENTS + 1
        ]),
    ] {
        f.set_page(
            "",
            vec![post("om_malformed", f.created - 1, false, "Title", items)],
            None,
        );
        assert!(f.read_with_media(None, true).await.is_err());
    }
}
