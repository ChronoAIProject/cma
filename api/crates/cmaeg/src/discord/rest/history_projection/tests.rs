use super::*;
use serde_json::json;
fn row(id: u64) -> Value {
    json!({"id":id.to_string(),"channel_id":"99","type":0,"author":{"id":"51","username":"Name"},"content":"Earlier", "timestamp":"2026-10-28T00:00:00Z","attachments":[]})
}
#[test]
fn exact_page_limits_and_sparse_progress_follow_processed_raw_rows() {
    for count in [0, 1, 25, 26] {
        let rows = json!((0..count).map(|i| row(699 - i)).collect::<Vec<_>>());
        let page = project(&rows, "99", "700").unwrap();
        assert_eq!(page.scanned, count.min(25) as u8);
        assert_eq!(page.next.is_some(), count == 26);
        if count > 0 {
            assert_eq!(page.messages.last().unwrap().message_id, "699");
        }
    }
    let rows = json!((0..26)
        .map(|i| {
            let mut r = row(699 - i);
            r["type"] = 6.into();
            r
        })
        .collect::<Vec<_>>());
    let page = project(&rows, "99", "700").unwrap();
    assert!(page.messages.is_empty());
    assert_eq!(page.omitted, 25);
    assert_eq!(page.next.as_deref(), Some("675"));
}
#[test]
fn refuses_duplicate_foreign_out_of_order_newer_and_invalid_lookahead() {
    for case in 0..7 {
        let mut rows = vec![row(699), row(698)];
        match case {
            0 => rows[1]["id"] = "699".into(),
            1 => rows[1]["channel_id"] = "88".into(),
            2 => rows.swap(0, 1),
            3 => rows[0]["id"] = "700".into(),
            4 => rows[0]["id"] = "18446744073709551616".into(),
            5 => rows = (0..27).map(|i| row(699 - i)).collect(),
            _ => {
                rows = (0..26).map(|i| row(699 - i)).collect();
                rows[25]["attachments"] = json!([{"id":"bad"}]);
            }
        }
        assert!(project(&json!(rows), "99", "700").is_err(), "case {case}");
    }
}
#[test]
fn clips_unicode_metadata_and_projects_only_attachment_facts_with_truthful_identity() {
    let rows=json!((0..25).map(|i| {
        let mut r=row(699-i);r["content"]="界".repeat(4000).into();r["author"]["username"]="名".repeat(300).into();
        r["attachments"]=json!([{"id":"66","filename":"文".repeat(1000),"size":42,"content_type":"image/png","description":"字".repeat(5000),"width":20,"height":30,"duration_secs":1.5,"ephemeral":true,"url":"https://secret-file.example","proxy_url":"https://secret-proxy.example","waveform":"SECRET","reference":"SECRET","source":"SECRET"}]);
        r["embeds"]=json!([{"url":"https://nested.example"}]);r["components"]=json!([{}]);r["sticker_items"]=json!([{}]);r["poll"]=json!({});r["message_snapshots"]=json!([{"message":{"content":"SECRET"}}]);
        if i==0 {r["webhook_id"]="77".into();} else if i==1 {r["author"]["bot"]=true.into();} else if i==2 {r["author"]["system"]=true.into();} r
    }).collect::<Vec<_>>());
    let page = project(&rows, "99", "700").unwrap();
    assert!(page.text_truncated && page.metadata_truncated);
    assert!(page.messages.iter().map(|m| m.text.len()).sum::<usize>() <= HISTORY_TEXT_BYTES);
    assert_eq!(page.messages[24].author.kind, ActorKind::Unknown);
    assert_eq!(page.messages[23].author.kind, ActorKind::Bot);
    assert_eq!(page.messages[22].author.kind, ActorKind::Unknown);
    assert_eq!(page.messages[24].omitted_content.len(), 5);
    let wire = serde_json::to_string(&page.messages).unwrap();
    assert!(!wire.contains("SECRET") && !wire.contains("https://") && !wire.contains("waveform"));
    assert_eq!(page.messages[24].attachments[0].duration_seconds, Some(1.5));
}
#[test]
fn malformed_optional_facts_refuse_the_whole_page() {
    for (key, value) in [
        ("duration_secs", json!(-1)),
        ("width", json!(-1)),
        ("height", json!(1.1)),
        ("ephemeral", json!("yes")),
        ("size", json!(-1)),
        ("content_type", json!(4)),
    ] {
        let mut attachment = json!({"id":"66","filename":"a","size":4});
        attachment[key] = value;
        let mut r = row(699);
        r["attachments"] = json!([attachment]);
        assert!(project(&json!([r]), "99", "700").is_err(), "{key}");
    }
    let mut r = row(699);
    r["content"] = "".into();
    r["attachments"] = json!([]);
    assert!(project(&json!([r]), "99", "700").unwrap().messages[0]
        .text
        .is_empty());
}
